use std::cell::RefCell;
use std::{collections::BTreeMap, rc::Rc};

#[allow(
    renamed_and_removed_lints,
    mismatched_lifetime_syntaxes,
    unexpected_cfgs,
    unused,
    clippy::panic,
    clippy::unwrap_used,
    clippy::all
)]
mod envoy;

mod services;

trait Service {
    type Response;
    fn dispatch(&self, ctx: &mut ReqRespCtx) -> usize;
    fn parse_message(&self, message: Vec<u8>) -> Self::Response;
}

struct FakeService {}

impl Service for FakeService {
    type Response = bool;

    fn dispatch(&self, ctx: &mut ReqRespCtx) -> usize {
        ctx.next_token_id()
    }
    fn parse_message(&self, mut message: Vec<u8>) -> bool {
        Some(1u8) == message.pop()
    }
}

struct Pipeline {
    ctx: ReqRespCtx,
    todos: Vec<Box<dyn Task>>,
    pending_tasks: BTreeMap<usize, PendingTask>,
}

impl Pipeline {
    fn eval(mut self) -> Option<Self> {
        let mut todos = Vec::default();
        for task in self.todos.drain(..) {
            match task.apply(&mut self.ctx) {
                TaskOutcome::Done => {}
                TaskOutcome::Deferred((token_id, t)) => {
                    if self.pending_tasks.insert(token_id, t).is_some() {
                        panic!("Duplicate token_id={}", token_id);
                    }
                }
                TaskOutcome::Pending(action) => todos.push(action),
            }
        }
        self.todos = todos;
        if self.pending_tasks.is_empty() && self.todos.is_empty() {
            None
        } else {
            Some(self)
        }
    }

    fn digest(&mut self, token_id: usize, response: Vec<u8>) {
        if let Some(pending) = self.pending_tasks.remove(&token_id) {
            // Process the response
            if let Some(action) = pending.process_response(response) {
                match action.apply(&mut self.ctx) {
                    TaskOutcome::Done => {}
                    TaskOutcome::Deferred((token_id, pending_task)) => {
                        if self.pending_tasks.insert(token_id, pending_task).is_some() {
                            panic!("Duplicate token_id={}", token_id);
                        }
                    }
                    TaskOutcome::Pending(action) => self.todos.push(action),
                }
            };
        } else {
            panic!("token_id={} not found", token_id);
        }
    }

    fn is_blocked(&self) -> bool {
        self.pending_tasks.values().any(PendingTask::is_blocking)
    }
}

#[cfg(test)]
impl Drop for Pipeline {
    fn drop(&mut self) {
        if !self.todos.is_empty() || !self.pending_tasks.is_empty() {
            panic!("Pipeline dropped with pending tasks");
        }
    }
}

struct RLTask {
    predicate: Predicate,
    service: Rc<dyn Service<Response = bool>>,
    allow_task: Option<Box<dyn Task>>,
    deny_task: Box<dyn Task>,
}

impl Task for RLTask {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        match self.predicate.eval(ctx) {
            PendingValue::Resolved(exec) => {
                if exec {
                    let token_id: usize = self.service.dispatch(ctx);
                    TaskOutcome::Deferred((
                        token_id,
                        PendingTask {
                            is_blocking: true,
                            allow_task: self.allow_task,
                            deny_task: self.deny_task,
                            service: self.service,
                        },
                    ))
                } else {
                    TaskOutcome::Done
                }
            }
            PendingValue::Pending => TaskOutcome::Pending(self),
        }
    }
}

struct PendingTask {
    is_blocking: bool,
    allow_task: Option<Box<dyn Task>>,
    deny_task: Box<dyn Task>,
    service: Rc<dyn Service<Response = bool>>,
}

impl PendingTask {
    fn process_response(self, response: Vec<u8>) -> Option<Box<dyn Task>> {
        if self.service.parse_message(response) {
            Some(self.deny_task)
        } else if let Some(action) = self.allow_task {
            Some(action)
        } else {
            None
        }
    }

    fn is_blocking(&self) -> bool {
        // This would need to peak into `ok_action` AND `rl_action` to see if we need to block
        self.is_blocking
    }
}

enum TaskOutcome {
    Done,
    Deferred((usize, PendingTask)),
    Pending(Box<dyn Task>),
}

trait Task {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome;
}

#[derive(Clone)]
struct AddResponseHeadersTask {
    headers: Vec<(String, String)>,
}

impl Task for AddResponseHeadersTask {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        if *ctx.test_current_phase.borrow() == Some(Phase::ResponseHeaders) {
            ctx.response_headers = self.headers.clone();
            TaskOutcome::Done
        } else {
            TaskOutcome::Pending(self)
        }
    }
}

struct TooManyRequestsTask {}

impl Task for TooManyRequestsTask {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> TaskOutcome {
        ctx.status_code = Some(429);
        TaskOutcome::Done
    }
}

#[derive(Debug, PartialEq)]
enum PendingValue<T> {
    Resolved(T),
    Pending,
}

struct Predicate {}

impl Predicate {
    fn eval(&self, ctx: &mut ReqRespCtx) -> PendingValue<bool> {
        ctx.test_pop_predicate_value()
    }
}

#[derive(Eq, PartialEq)]
enum Phase {
    RequestHeaders,
    RequestBody,
    ResponseHeaders,
    ResponseBody,
}

#[derive(Default)]
struct ReqRespCtx {
    test_token_id: usize,
    test_current_phase: Rc<RefCell<Option<Phase>>>,
    test_predicate_values: Vec<PendingValue<bool>>,
    status_code: Option<u32>,
    response_headers: Vec<(String, String)>,
}

impl ReqRespCtx {
    fn test_pop_predicate_value(&mut self) -> PendingValue<bool> {
        self.test_predicate_values.pop().expect("Expected a value")
    }

    fn next_token_id(&mut self) -> usize {
        self.test_token_id += 1;
        self.test_token_id
    }

    fn get_attribute(&self, key: &str) -> PendingValue<Option<String>> {
        match key {
            "ratelimit.domain" => PendingValue::Resolved(Some("example".to_string())),
            _ => PendingValue::Resolved(None),
        }
    }
}

mod tests {
    use super::*;

    #[test]
    fn it_rate_limits() {
        // on_request_headers() {
        let mut ctx = ReqRespCtx::default();
        ctx.test_predicate_values.push(PendingValue::Resolved(true));
        let mut pipeline = Pipeline {
            ctx,
            todos: vec![Box::new(RLTask {
                predicate: Predicate {},
                service: Rc::new(FakeService {}),
                allow_task: None,
                deny_task: Box::new(TooManyRequestsTask {}),
            })],
            pending_tasks: Default::default(),
        };

        pipeline = pipeline
            .eval()
            .expect("Pipeline should be waiting for limitador");
        assert!(pipeline.is_blocked(), "Filter should be paused");

        // fn on_grpc_call_response(&mut self, token_id: u32, status_code: u32, resp_size: usize) {
        let buffer: Vec<u8> = vec![1u8];
        let token_id = 1;
        pipeline.digest(token_id, buffer);
        assert!(!pipeline.is_blocked(), "Filter should be continued");
        assert_eq!(pipeline.ctx.status_code, Some(429));

        // on_request_body() {

        // on_response_headers() {

        // on_response_body() {
        // pipeline.eval();
    }

    #[test]
    fn it_not_rate_limits() {
        // on_request_headers() {
        let mut ctx = ReqRespCtx::default();
        let rc = Rc::new(RefCell::new(Some(Phase::RequestHeaders)));
        ctx.test_current_phase = rc.clone();
        ctx.test_predicate_values.push(PendingValue::Resolved(true));
        let mut pipeline = Pipeline {
            ctx: ctx,
            todos: vec![Box::new(RLTask {
                predicate: Predicate {},
                service: Rc::new(FakeService {}),
                allow_task: Some(Box::new(AddResponseHeadersTask {
                    headers: vec![("X-RateLimit-Limit".to_string(), "10".to_string())],
                })),
                deny_task: Box::new(TooManyRequestsTask {}),
            })],
            pending_tasks: Default::default(),
        };

        pipeline = pipeline
            .eval()
            .expect("Pipeline should be waiting for limitador");
        assert!(pipeline.is_blocked(), "Filter should be paused");

        // fn on_grpc_call_response(&mut self, token_id: u32, status_code: u32, resp_size: usize) {
        let buffer: Vec<u8> = Vec::new();
        let token_id = 1;
        pipeline.digest(token_id, buffer);
        assert!(!pipeline.is_blocked(), "Filter should be continued");
        assert_eq!(pipeline.ctx.status_code, None);
        assert!(
            pipeline.ctx.response_headers.is_empty(),
            "Headers should be empty"
        );

        // on_request_body() {
        pipeline = pipeline.eval().expect("Not done yet");
        assert!(
            pipeline.ctx.response_headers.is_empty(),
            "Headers should be empty"
        );

        // on_response_headers() {
        rc.replace(Some(Phase::ResponseHeaders));
        assert!(pipeline.eval().is_none(), "Done now");
        // assert_eq!(
        //     pipeline.ctx.headers,
        //     vec![("x".to_string(), "y".to_string())]
        // );

        // on_response_body() {
        // pipeline.eval();
    }

    #[test]
    fn it_token_rate_limits() {
        // on_request_headers() {
        let mut ctx = ReqRespCtx::default();
        let rc = Rc::new(RefCell::new(Some(Phase::RequestHeaders)));
        ctx.test_current_phase = rc.clone();
        ctx.test_predicate_values
            .insert(0, PendingValue::Resolved(true));
        ctx.test_predicate_values.insert(0, PendingValue::Pending);
        ctx.test_predicate_values.insert(0, PendingValue::Pending);
        ctx.test_predicate_values.insert(0, PendingValue::Pending);
        ctx.test_predicate_values
            .insert(0, PendingValue::Resolved(true));
        let mut pipeline = Pipeline {
            ctx: ctx,
            todos: vec![
                Box::new(RLTask {
                    predicate: Predicate {},
                    service: Rc::new(FakeService {}),
                    allow_task: None,
                    deny_task: Box::new(TooManyRequestsTask {}),
                }),
                Box::new(RLTask {
                    predicate: Predicate {},
                    service: Rc::new(FakeService {}),
                    allow_task: None,
                    deny_task: Box::new(TooManyRequestsTask {}),
                }),
            ],
            pending_tasks: Default::default(),
        };

        pipeline = pipeline
            .eval()
            .expect("Pipeline should be waiting for limitador");
        assert!(pipeline.is_blocked(), "Filter should be paused");

        // fn on_grpc_call_response(&mut self, token_id: u32, status_code: u32, resp_size: usize) {
        let buffer: Vec<u8> = Vec::new();
        let token_id = 1;
        pipeline.digest(token_id, buffer);
        assert!(!pipeline.is_blocked(), "Filter should be continued");
        assert_eq!(pipeline.ctx.status_code, None);

        // on_request_body() {
        pipeline = pipeline.eval().expect("Not done yet");

        // on_response_headers() {
        pipeline = pipeline.eval().expect("Not done yet");
        // assert_eq!(
        //     pipeline.ctx.headers,
        //     vec![("x".to_string(), "y".to_string())]
        // );

        // on_response_body() {
        pipeline = pipeline.eval().expect("Not done yet");

        // on_grpc_response
        pipeline.digest(2, vec![1u8]);
    }

    // #[test]
    pub fn it_gets_attributes() {
        let ctx = ReqRespCtx::default();
        assert_eq!(
            ctx.get_attribute("doesntexist"),
            PendingValue::Resolved(None)
        );
        assert_eq!(
            ctx.get_attribute("ratelimit.domain"),
            PendingValue::Resolved(Some("example".to_string()))
        );
    }
}

fn main() {
    // tests::it_rate_limits();
    // tests::it_not_rate_limits();
    // tests::it_token_rate_limits();
    // tests::it_gets_attributes();
    // println!("ok")
}
