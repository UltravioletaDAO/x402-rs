//! A scripted Sui gRPC node for tests: the three services the provider calls,
//! each answering from a closure, every call recorded.
//!
//! Served by a real tonic server on a loopback port and reached through the
//! real `sui-rpc` client, so the statuses the provider classifies are the ones
//! tonic actually delivers, not values written to match.

use std::sync::{Arc, Mutex};

use sui_rpc::proto::sui::rpc::v2::{
    ledger_service_server::{LedgerService, LedgerServiceServer},
    state_service_server::{StateService, StateServiceServer},
    transaction_execution_service_server::{
        TransactionExecutionService, TransactionExecutionServiceServer,
    },
    BatchGetObjectsRequest, BatchGetObjectsResponse, ExecuteTransactionRequest,
    ExecuteTransactionResponse, GetBalanceRequest, GetBalanceResponse, GetObjectRequest,
    GetObjectResponse, GetTransactionRequest, GetTransactionResponse,
};
use tonic::{Request, Response, Status};

/// What a scripted method does with one call.
pub enum Reply<T> {
    Answer(Result<T, Status>),
    /// Never answer (the client's own timeout has to end the call).
    Hang,
}

type Handler<Req, Resp> = Box<dyn Fn(&Req) -> Reply<Resp> + Send + Sync>;

pub struct Node {
    pub batch_get_objects: Handler<BatchGetObjectsRequest, BatchGetObjectsResponse>,
    pub get_object: Handler<GetObjectRequest, GetObjectResponse>,
    pub get_balance: Handler<GetBalanceRequest, GetBalanceResponse>,
    pub get_transaction: Handler<GetTransactionRequest, GetTransactionResponse>,
    pub execute: Handler<ExecuteTransactionRequest, ExecuteTransactionResponse>,
    calls: Mutex<Vec<&'static str>>,
    executions: Mutex<Vec<ExecuteTransactionRequest>>,
}

fn unscripted<Req, Resp>() -> Handler<Req, Resp> {
    Box::new(|_| Reply::Answer(Err(Status::unimplemented("not scripted"))))
}

impl Default for Node {
    fn default() -> Self {
        Self {
            batch_get_objects: unscripted(),
            get_object: unscripted(),
            get_balance: unscripted(),
            get_transaction: unscripted(),
            execute: unscripted(),
            calls: Mutex::new(Vec::new()),
            executions: Mutex::new(Vec::new()),
        }
    }
}

impl Node {
    /// Every call this node received, in order, by method name.
    pub fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }

    pub fn count(&self, method: &str) -> usize {
        self.calls().iter().filter(|m| **m == method).count()
    }

    /// Every `ExecuteTransaction` request this node received.
    pub fn executions(&self) -> Vec<ExecuteTransactionRequest> {
        self.executions.lock().unwrap().clone()
    }

    fn record(&self, method: &'static str) {
        self.calls.lock().unwrap().push(method);
    }
}

#[derive(Clone)]
struct Service(Arc<Node>);

async fn reply<T>(reply: Reply<T>) -> Result<Response<T>, Status> {
    match reply {
        Reply::Answer(result) => result.map(Response::new),
        Reply::Hang => {
            tokio::time::sleep(std::time::Duration::from_secs(3_600)).await;
            Err(Status::deadline_exceeded("hung"))
        }
    }
}

#[tonic::async_trait]
impl LedgerService for Service {
    async fn get_object(
        &self,
        request: Request<GetObjectRequest>,
    ) -> Result<Response<GetObjectResponse>, Status> {
        self.0.record("GetObject");
        reply((self.0.get_object)(request.get_ref())).await
    }

    async fn batch_get_objects(
        &self,
        request: Request<BatchGetObjectsRequest>,
    ) -> Result<Response<BatchGetObjectsResponse>, Status> {
        self.0.record("BatchGetObjects");
        reply((self.0.batch_get_objects)(request.get_ref())).await
    }

    async fn get_transaction(
        &self,
        request: Request<GetTransactionRequest>,
    ) -> Result<Response<GetTransactionResponse>, Status> {
        self.0.record("GetTransaction");
        reply((self.0.get_transaction)(request.get_ref())).await
    }
}

#[tonic::async_trait]
impl StateService for Service {
    async fn get_balance(
        &self,
        request: Request<GetBalanceRequest>,
    ) -> Result<Response<GetBalanceResponse>, Status> {
        self.0.record("GetBalance");
        reply((self.0.get_balance)(request.get_ref())).await
    }
}

#[tonic::async_trait]
impl TransactionExecutionService for Service {
    async fn execute_transaction(
        &self,
        request: Request<ExecuteTransactionRequest>,
    ) -> Result<Response<ExecuteTransactionResponse>, Status> {
        self.0.record("ExecuteTransaction");
        self.0
            .executions
            .lock()
            .unwrap()
            .push(request.get_ref().clone());
        reply((self.0.execute)(request.get_ref())).await
    }
}

/// Serve `node` on a loopback port; its URL and a handle to read its calls.
pub async fn serve(node: Node) -> (String, Arc<Node>) {
    let node = Arc::new(node);
    let service = Service(node.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(LedgerServiceServer::new(service.clone()))
            .add_service(StateServiceServer::new(service.clone()))
            .add_service(TransactionExecutionServiceServer::new(service))
            .serve_with_incoming(tonic::transport::server::TcpIncoming::from(listener)),
    );
    (format!("http://{addr}"), node)
}

/// A loopback URL nothing listens on: every connection is refused.
pub async fn closed_endpoint() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}")
}
