//! This is a layer for [tracing] to capture the state transition of deploys
//!
//! The idea is as follow: as a deployment moves through the [super::DeploymentManager] a set of functions will be invoked.
//! These functions are clear markers for the deployment entering a new state so we would want to change the state as soon as entering these functions.
//! But rather than passing a persistence layer around to be able record the state in these functions we can rather use [tracing].
//!
//! This is very similar to Aspect Oriented Programming where we use the annotations from the function to trigger the recording of a new state.
//! This annotation is a [#[instrument]](https://docs.rs/tracing-attributes/latest/tracing_attributes/attr.instrument.html) with an `id` and `state` field as follow:
//! ```no-test
//! #[instrument(fields(deployment_id = %built.id, state = %State::Built))]
//! pub async fn new_state_fn(built: Built) {
//!     // Get built ready for starting
//! }
//! ```
//!
//! Here the `id` is extracted from the `built` argument and the `state` is taken from the [State] enum (the special `%` is needed to use the `Display` trait to convert the values to a str).
//!
//! **Warning** Don't log out sensitive info in functions with these annotations

use std::str::FromStr;

use tracing::{field::Visit, span, warn, Metadata, Subscriber};
use tracing_subscriber::Layer;
use uuid::Uuid;

use shuttle_common::{
    log::{Backend, ColoredLevel, LogRecorder},
    LogItem,
};

use crate::persistence::{DeploymentStateUpdate, State, StateRecorder};

/// Tracing subscriber layer which keeps track of a deployment's state.
/// Logs a special line when entering a span tagged with deployment id and state.
pub struct StateChangeLayer<R, SR>
where
    R: LogRecorder + Send + Sync,
    SR: StateRecorder + Send + Sync,
{
    pub log_recorder: R,
    pub state_recorder: SR,
}

impl<R, S, SR> Layer<S> for StateChangeLayer<R, SR>
where
    S: Subscriber + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>,
    R: LogRecorder + Send + Sync + 'static,
    SR: StateRecorder,
{
    fn on_new_span(
        &self,
        attrs: &span::Attributes<'_>,
        _id: &span::Id,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        // We only care about spans that change the state
        if !NewStateVisitor::is_valid(attrs.metadata()) {
            return;
        }

        let mut visitor = NewStateVisitor::default();
        attrs.record(&mut visitor);

        if visitor.deployment_id.is_nil() {
            warn!("scope details does not have a valid deployment id");
            return;
        }

        // To deployer persistence
        let res = self.state_recorder.record_state(DeploymentStateUpdate {
            id: visitor.deployment_id,
            state: visitor.state,
        });

        let log_line = match res {
            Ok(_) => format!(
                "{} Entering {} state",
                tracing::Level::INFO.colored(),
                visitor.state, // make blue?
            ),
            Err(_) => format!(
                "{} The deployer failed while recording the new state: {}",
                tracing::Level::ERROR.colored(),
                visitor.state
            ),
        };

        // To logger
        self.log_recorder.record(LogItem::new(
            visitor.deployment_id,
            Backend::Deployer,
            log_line,
        ));
    }
}

/// To extract `deployment_id` and `state` fields for scopes that have them
#[derive(Default)]
struct NewStateVisitor {
    deployment_id: Uuid,
    state: State,
}

impl NewStateVisitor {
    /// Field containing the deployment identifier
    const ID_IDENT: &'static str = "deployment_id";

    /// Field containing the deployment state identifier
    const STATE_IDENT: &'static str = "state";

    fn is_valid(metadata: &Metadata) -> bool {
        metadata.is_span()
            && metadata.fields().field(Self::ID_IDENT).is_some()
            && metadata.fields().field(Self::STATE_IDENT).is_some()
    }
}

impl Visit for NewStateVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == Self::STATE_IDENT {
            self.state = State::from_str(&format!("{value:?}")).unwrap_or_default();
        } else if field.name() == Self::ID_IDENT {
            self.deployment_id = Uuid::try_parse(&format!("{value:?}")).unwrap_or_default();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs::read_dir,
        net::{Ipv4Addr, SocketAddr},
        path::PathBuf,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use crate::{
        persistence::{
            resource::ResourceManager, DeploymentStateUpdate, DeploymentUpdater, StateRecorder,
        },
        RuntimeManager,
    };
    use async_trait::async_trait;
    use axum::body::Bytes;
    use ctor::ctor;
    use flate2::{write::GzEncoder, Compression};
    use portpicker::pick_unused_port;
    use shuttle_common::{claims::Claim, resource::ResourceType};
    use shuttle_common_tests::{builder::mocked_builder_client, logger::mocked_logger_client};
    use shuttle_proto::{
        builder::{builder_server::Builder, BuildRequest, BuildResponse},
        logger::{
            logger_client::LoggerClient, logger_server::Logger, Batcher, LogLine, LogsRequest,
            LogsResponse, StoreLogsRequest, StoreLogsResponse,
        },
        provisioner::{
            provisioner_server::{Provisioner, ProvisionerServer},
            ContainerRequest, ContainerResponse, DatabaseDeletionResponse, DatabaseRequest,
            DatabaseResponse, Ping, Pong,
        },
        resource_recorder::{ResourceResponse, ResourcesResponse, ResultResponse},
    };
    use tokio::{select, sync::mpsc, time::sleep};
    use tokio_stream::wrappers::ReceiverStream;
    use tonic::{transport::Server, Request, Response, Status};
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    use ulid::Ulid;
    use uuid::Uuid;

    use crate::{
        deployment::{
            gateway_client::BuildQueueClient, ActiveDeploymentsGetter, Built, DeploymentManager,
            Queued,
        },
        persistence::State,
    };

    use super::{LogItem, StateChangeLayer};

    use shuttle_common::log::LogRecorder;

    #[ctor]
    static RECORDER: RecorderMock = {
        let recorder = RecorderMock::new();

        // Copied from the test-log crate
        let event_filter = {
            use ::tracing_subscriber::fmt::format::FmtSpan;

            match ::std::env::var("RUST_LOG_SPAN_EVENTS") {
                Ok(value) => {
                    value
                        .to_ascii_lowercase()
                        .split(',')
                        .map(|filter| match filter.trim() {
                            "new" => FmtSpan::NEW,
                            "enter" => FmtSpan::ENTER,
                            "exit" => FmtSpan::EXIT,
                            "close" => FmtSpan::CLOSE,
                            "active" => FmtSpan::ACTIVE,
                            "full" => FmtSpan::FULL,
                            _ => panic!("test-log: RUST_LOG_SPAN_EVENTS must contain filters separated by `,`.\n\t\
                                         For example: `active` or `new,close`\n\t\
                                         Supported filters: new, enter, exit, close, active, full\n\t\
                                         Got: {}", value),
                        })
                        .fold(FmtSpan::NONE, |acc, filter| filter | acc)
                },
                Err(::std::env::VarError::NotUnicode(_)) =>
                    panic!("test-log: RUST_LOG_SPAN_EVENTS must contain a valid UTF-8 string"),
                Err(::std::env::VarError::NotPresent) => FmtSpan::NONE,
            }
        };
        let fmt_layer = fmt::layer()
            .with_test_writer()
            .with_span_events(event_filter);
        let filter_layer = EnvFilter::try_from_default_env()
            .or_else(|_| EnvFilter::try_new("shuttle_deployer"))
            .unwrap();

        tracing_subscriber::registry()
            .with(StateChangeLayer {
                log_recorder: recorder.clone(),
                state_recorder: recorder.clone(),
            })
            .with(filter_layer)
            .with(fmt_layer)
            .init();

        recorder
    };

    #[derive(Clone)]
    struct RecorderMock {
        states: Arc<Mutex<Vec<MockStateLog>>>,
    }

    #[derive(Clone, Debug, PartialEq)]
    struct MockStateLog {
        id: Uuid,
        state: State,
    }

    impl From<DeploymentStateUpdate> for MockStateLog {
        fn from(log: DeploymentStateUpdate) -> Self {
            Self {
                id: log.id,
                state: log.state,
            }
        }
    }

    impl RecorderMock {
        fn new() -> Self {
            Self {
                states: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn get_deployment_states(&self, id: &Uuid) -> Vec<MockStateLog> {
            self.states
                .lock()
                .unwrap()
                .iter()
                .filter(|log| log.id == *id)
                .cloned()
                .collect()
        }
    }

    impl LogRecorder for RecorderMock {
        fn record(&self, _: LogItem) {}
    }

    #[async_trait]
    impl Builder for RecorderMock {
        async fn build(
            &self,
            _request: tonic::Request<BuildRequest>,
        ) -> Result<tonic::Response<BuildResponse>, tonic::Status> {
            Ok(Response::new(BuildResponse::default()))
        }
    }

    #[derive(thiserror::Error, Debug)]
    pub enum MockError {}

    impl StateRecorder for RecorderMock {
        type Err = MockError;

        fn record_state(&self, event: DeploymentStateUpdate) -> Result<(), MockError> {
            self.states.lock().unwrap().push(event.into());
            Ok(())
        }
    }

    #[async_trait]
    impl Logger for RecorderMock {
        async fn store_logs(
            &self,
            _: Request<StoreLogsRequest>,
        ) -> Result<Response<StoreLogsResponse>, Status> {
            Ok(Response::new(StoreLogsResponse { success: true }))
        }

        async fn get_logs(
            &self,
            _: Request<LogsRequest>,
        ) -> Result<Response<LogsResponse>, Status> {
            Ok(Response::new(LogsResponse {
                log_items: Vec::new(),
            }))
        }

        type GetLogsStreamStream = ReceiverStream<Result<LogLine, Status>>;

        async fn get_logs_stream(
            &self,
            _: Request<LogsRequest>,
        ) -> Result<Response<Self::GetLogsStreamStream>, Status> {
            let (_, rx) = mpsc::channel(1);
            Ok(Response::new(ReceiverStream::new(rx)))
        }
    }

    struct ProvisionerMock;

    #[async_trait]
    impl Provisioner for ProvisionerMock {
        async fn provision_database(
            &self,
            _request: tonic::Request<DatabaseRequest>,
        ) -> Result<tonic::Response<DatabaseResponse>, tonic::Status> {
            panic!("no deploy layer tests should request a db");
        }

        async fn provision_arbitrary_container(
            &self,
            _req: tonic::Request<ContainerRequest>,
        ) -> Result<tonic::Response<ContainerResponse>, tonic::Status> {
            panic!("no deploy layer tests should request container")
        }

        async fn delete_database(
            &self,
            _request: tonic::Request<DatabaseRequest>,
        ) -> Result<tonic::Response<DatabaseDeletionResponse>, tonic::Status> {
            panic!("no deploy layer tests should request delete a db");
        }

        async fn health_check(
            &self,
            _request: tonic::Request<Ping>,
        ) -> Result<tonic::Response<Pong>, tonic::Status> {
            panic!("no run tests should do a health check");
        }
    }

    async fn get_runtime_manager(
        logger_client: Batcher<
            LoggerClient<
                shuttle_common::claims::ClaimService<
                    shuttle_common::claims::InjectPropagation<tonic::transport::Channel>,
                >,
            >,
        >,
    ) -> Arc<tokio::sync::Mutex<RuntimeManager>> {
        let provisioner_addr =
            SocketAddr::new(Ipv4Addr::LOCALHOST.into(), pick_unused_port().unwrap());
        tokio::spawn(async move {
            let mock = ProvisionerMock;
            Server::builder()
                .add_service(ProvisionerServer::new(mock))
                .serve(provisioner_addr)
                .await
                .unwrap();
        });

        RuntimeManager::new(format!("http://{}", provisioner_addr), logger_client, None)
    }

    #[derive(Clone)]
    struct StubDeploymentUpdater;

    #[async_trait::async_trait]
    impl DeploymentUpdater for StubDeploymentUpdater {
        type Err = std::io::Error;

        async fn set_address(&self, _id: &Uuid, _address: &SocketAddr) -> Result<(), Self::Err> {
            Ok(())
        }

        async fn set_is_next(&self, _id: &Uuid, _is_next: bool) -> Result<(), Self::Err> {
            Ok(())
        }
    }

    #[derive(Clone)]
    struct StubActiveDeploymentGetter;

    #[async_trait::async_trait]
    impl ActiveDeploymentsGetter for StubActiveDeploymentGetter {
        type Err = std::io::Error;

        async fn get_active_deployments(
            &self,
            _service_id: &Ulid,
        ) -> std::result::Result<Vec<Uuid>, Self::Err> {
            Ok(vec![])
        }
    }

    #[derive(Clone)]
    struct StubBuildQueueClient;

    #[async_trait::async_trait]
    impl BuildQueueClient for StubBuildQueueClient {
        async fn get_slot(
            &self,
            _id: Uuid,
        ) -> Result<bool, shuttle_common::backends::client::Error> {
            Ok(true)
        }

        async fn release_slot(
            &self,
            _id: Uuid,
        ) -> Result<(), shuttle_common::backends::client::Error> {
            Ok(())
        }
    }

    #[derive(Clone)]
    struct StubResourceManager;

    #[async_trait]
    impl ResourceManager for StubResourceManager {
        type Err = std::io::Error;

        async fn insert_resources(
            &mut self,
            _resource: Vec<shuttle_proto::resource_recorder::record_request::Resource>,
            _service_id: &ulid::Ulid,
            _claim: Claim,
        ) -> Result<ResultResponse, Self::Err> {
            Ok(ResultResponse {
                success: true,
                message: "dummy impl".to_string(),
            })
        }
        async fn get_resources(
            &mut self,
            _service_id: &ulid::Ulid,
            _claim: Claim,
        ) -> Result<ResourcesResponse, Self::Err> {
            Ok(ResourcesResponse {
                success: true,
                message: "dummy impl".to_string(),
                resources: Vec::new(),
            })
        }

        async fn get_resource(
            &mut self,
            _service_id: &ulid::Ulid,
            _type: ResourceType,
            _claim: Claim,
        ) -> Result<ResourceResponse, Self::Err> {
            Ok(ResourceResponse {
                success: true,
                message: "dummy impl".to_string(),
                resource: None,
            })
        }

        async fn delete_resource(
            &mut self,
            _project_name: String,
            _service_id: &ulid::Ulid,
            _type: ResourceType,
            _claim: Claim,
        ) -> Result<ResultResponse, Self::Err> {
            Ok(ResultResponse {
                success: true,
                message: "dummy impl".to_string(),
            })
        }
    }

    async fn test_states(id: &Uuid, expected_states: Vec<MockStateLog>) {
        loop {
            let states = RECORDER.get_deployment_states(id);
            if states == expected_states {
                return;
            }

            for (actual, expected) in states.iter().zip(&expected_states) {
                if actual != expected {
                    return;
                }
            }

            sleep(Duration::from_millis(250)).await;
        }
    }

    const STATE_TEST_TIMEOUT_SECS: u64 = 600;

    #[tokio::test(flavor = "multi_thread")]
    async fn deployment_to_be_queued() {
        let deployment_manager = get_deployment_manager().await;

        let queued = get_queue("sleep-async");
        let id = queued.id;
        deployment_manager.queue_push(queued).await;

        let test = test_states(
            &id,
            vec![
                MockStateLog {
                    id,
                    state: State::Queued,
                },
                MockStateLog {
                    id,
                    state: State::Building,
                },
                MockStateLog {
                    id,
                    state: State::Built,
                },
                MockStateLog {
                    id,
                    state: State::Loading,
                },
                MockStateLog {
                    id,
                    state: State::Running,
                },
            ],
        );

        select! {
            _ = sleep(Duration::from_secs(STATE_TEST_TIMEOUT_SECS)) => {
                let states = RECORDER.get_deployment_states(&id);
                panic!("states should go into 'Running' for a valid service: {:#?}", states);
            },
            _ = test => {}
        };

        // Send kill signal
        deployment_manager.kill(id).await;

        let test = test_states(
            &id,
            vec![
                MockStateLog {
                    id,
                    state: State::Queued,
                },
                MockStateLog {
                    id,
                    state: State::Building,
                },
                MockStateLog {
                    id,
                    state: State::Built,
                },
                MockStateLog {
                    id,
                    state: State::Loading,
                },
                MockStateLog {
                    id,
                    state: State::Running,
                },
                MockStateLog {
                    id,
                    state: State::Stopped,
                },
                MockStateLog {
                    id,
                    state: State::Stopped,
                },
            ],
        );

        select! {
            _ = sleep(Duration::from_secs(60)) => {
                let states = RECORDER.get_deployment_states(&id);
                panic!("states should go into 'Stopped' for a valid service: {:#?}", states);
            },
            _ = test => {}
        };
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn deployment_self_stop() {
        let deployment_manager = get_deployment_manager().await;

        let queued = get_queue("self-stop");
        let id = queued.id;
        deployment_manager.queue_push(queued).await;

        let test = test_states(
            &id,
            vec![
                MockStateLog {
                    id,
                    state: State::Queued,
                },
                MockStateLog {
                    id,
                    state: State::Building,
                },
                MockStateLog {
                    id,
                    state: State::Built,
                },
                MockStateLog {
                    id,
                    state: State::Loading,
                },
                MockStateLog {
                    id,
                    state: State::Running,
                },
                MockStateLog {
                    id,
                    state: State::Completed,
                },
            ],
        );

        select! {
            _ = sleep(Duration::from_secs(STATE_TEST_TIMEOUT_SECS)) => {
                let states = RECORDER.get_deployment_states(&id);
                panic!("states should go into 'Completed' when a service stops by itself: {:#?}", states);
            }
            _ = test => {}
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn deployment_bind_panic() {
        let deployment_manager = get_deployment_manager().await;

        let queued = get_queue("bind-panic");
        let id = queued.id;
        deployment_manager.queue_push(queued).await;

        let test = test_states(
            &id,
            vec![
                MockStateLog {
                    id,
                    state: State::Queued,
                },
                MockStateLog {
                    id,
                    state: State::Building,
                },
                MockStateLog {
                    id,
                    state: State::Built,
                },
                MockStateLog {
                    id,
                    state: State::Loading,
                },
                MockStateLog {
                    id,
                    state: State::Running,
                },
                MockStateLog {
                    id,
                    state: State::Crashed,
                },
            ],
        );

        select! {
            _ = sleep(Duration::from_secs(STATE_TEST_TIMEOUT_SECS)) => {
                let states = RECORDER.get_deployment_states(&id);
                panic!("states should go into 'Crashed' panicking in bind: {:#?}", states);
            }
            _ = test => {}
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn deployment_main_panic() {
        let deployment_manager = get_deployment_manager().await;

        let queued = get_queue("main-panic");
        let id = queued.id;
        deployment_manager.queue_push(queued).await;

        let test = test_states(
            &id,
            vec![
                MockStateLog {
                    id,
                    state: State::Queued,
                },
                MockStateLog {
                    id,
                    state: State::Building,
                },
                MockStateLog {
                    id,
                    state: State::Built,
                },
                MockStateLog {
                    id,
                    state: State::Loading,
                },
                MockStateLog {
                    id,
                    state: State::Crashed,
                },
            ],
        );

        select! {
            _ = sleep(Duration::from_secs(STATE_TEST_TIMEOUT_SECS)) => {
                let states = RECORDER.get_deployment_states(&id);
                panic!("states should go into 'Crashed' when panicking in main: {:#?}", states);
            }
            _ = test => {}
        }
    }

    #[tokio::test]
    async fn deployment_from_run() {
        let deployment_manager = get_deployment_manager().await;

        let id = Uuid::new_v4();
        deployment_manager
            .run_push(Built {
                id,
                service_name: "run-test".to_string(),
                service_id: Ulid::new(),
                project_id: Ulid::new(),
                tracing_context: Default::default(),
                is_next: false,
                claim: Default::default(),
                secrets: Default::default(),
            })
            .await;

        let test = test_states(
            &id,
            vec![
                MockStateLog {
                    id,
                    state: State::Built,
                },
                MockStateLog {
                    id,
                    state: State::Loading,
                },
                MockStateLog {
                    id,
                    state: State::Crashed,
                },
            ],
        );

        select! {
            _ = sleep(Duration::from_secs(50)) => {
                let states = RECORDER.get_deployment_states(&id);
                panic!("from running should start in built and end in crash for invalid: {:#?}", states)
            },
            _ = test => {}
        };
    }

    #[tokio::test]
    async fn scope_with_nil_id() {
        let deployment_manager = get_deployment_manager().await;

        let id = Uuid::nil();
        deployment_manager
            .queue_push(Queued {
                id,
                service_name: "nil_id".to_string(),
                service_id: Ulid::new(),
                project_id: Ulid::new(),
                data: Bytes::from("violets are red").to_vec(),
                will_run_tests: false,
                tracing_context: Default::default(),
                claim: Default::default(),
            })
            .await;

        // Give it a small time to start up
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let states = RECORDER.get_deployment_states(&id);

        assert!(
            states.is_empty(),
            "no logs should be recorded when the scope id is invalid:\n\t{states:#?}"
        );
    }

    async fn get_deployment_manager() -> DeploymentManager {
        let logger_client = mocked_logger_client(RecorderMock::new()).await;
        let builder_client = mocked_builder_client(RecorderMock::new()).await;
        DeploymentManager::builder()
            .build_log_recorder(RECORDER.clone())
            .active_deployment_getter(StubActiveDeploymentGetter)
            .artifacts_path(PathBuf::from("/tmp"))
            .resource_manager(StubResourceManager)
            .log_fetcher(logger_client.clone())
            .builder_client(Some(builder_client))
            .runtime(get_runtime_manager(Batcher::wrap(logger_client)).await)
            .deployment_updater(StubDeploymentUpdater)
            .queue_client(StubBuildQueueClient)
            .build()
    }

    fn get_queue(name: &str) -> Queued {
        let enc = GzEncoder::new(Vec::new(), Compression::fast());
        let mut tar = tar::Builder::new(enc);

        for dir_entry in read_dir(format!("tests/deploy_layer/{name}")).unwrap() {
            let dir_entry = dir_entry.unwrap();
            if dir_entry.file_name() != "target" {
                let path = format!("{name}/{}", dir_entry.file_name().to_str().unwrap());

                if dir_entry.file_type().unwrap().is_dir() {
                    tar.append_dir_all(path, dir_entry.path()).unwrap();
                } else {
                    tar.append_path_with_name(dir_entry.path(), path).unwrap();
                }
            }
        }

        let enc = tar.into_inner().unwrap();
        let bytes = enc.finish().unwrap();

        println!("{name}: finished getting archive for test");

        Queued {
            id: Uuid::new_v4(),
            service_name: format!("deploy-layer-{name}"),
            service_id: Ulid::new(),
            project_id: Ulid::new(),
            data: bytes,
            will_run_tests: false,
            tracing_context: Default::default(),
            claim: Default::default(),
        }
    }
}
