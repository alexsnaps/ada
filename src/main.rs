use crate::PendingValue::Revolved;
use crate::Phase::ResponseHeaders;
use std::cell::RefCell;
use std::{collections::BTreeMap, rc::Rc};

enum Either<T, U> {
    Left(T),
    Right(U),
}

struct Service {}

impl Service {
    pub(crate) fn dispatch(&self, ctx: &mut ReqRespCtx) -> usize {
        ctx.next_token_id()
    }
    fn parse_message(&self, mut message: Vec<u8>) -> bool {
        Some(1u8) == message.pop()
    }
}

struct Pipeline {
    ctx: ReqRespCtx,
    todos: Vec<RLTask>,
    pending_tasks: BTreeMap<usize, PendingTask>,
    pending_actions: Vec<Box<dyn Action>>,
}

impl Pipeline {
    fn eval(mut self) -> Option<Self> {
        self.pending_actions
            .retain(|action| !action.apply(&mut self.ctx));

        let mut todos = Vec::default();
        for task in self.todos.drain(..) {
            let either = task.exec(&mut self.ctx);
            match either {
                Either::Left(None) => {}
                Either::Left(Some((token_id, t))) => {
                    if self.pending_tasks.contains_key(&token_id) {
                        panic!("NONONONONO")
                    }
                    self.pending_tasks.insert(token_id, t);
                }
                Either::Right(todo) => {
                    todos.push(todo);
                }
            }
        }
        self.todos = todos;
        if self.pending_tasks.is_empty() && self.pending_actions.is_empty() && self.todos.is_empty()
        {
            None
        } else {
            Some(self)
        }
    }

    fn digest(&mut self, token_id: usize, response: Vec<u8>) {
        if let Some(pending) = self.pending_tasks.remove(&token_id) {
            // Process the response
            if let Some(action) = pending.process_response(response)
                && !action.apply(&mut self.ctx)
            {
                self.pending_actions.push(action);
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
        if !self.todos.is_empty()
            || !self.pending_tasks.is_empty()
            || !self.pending_actions.is_empty()
        {
            panic!("Pipeline dropped with pending tasks");
        }
    }
}

struct RLTask {
    predicate: Predicate,
    service: Rc<Service>,
    ok_action: Option<AddResponseHeadersAction>,
    rl_action: Box<dyn Action>,
}

impl RLTask {
    fn exec(self, ctx: &mut ReqRespCtx) -> Either<Option<(usize, PendingTask)>, Self> {
        match self.predicate.eval(ctx) {
            Revolved(exec) => {
                if exec {
                    let token_id: usize = self.service.dispatch(ctx);
                    Either::Left(Some((
                        token_id,
                        PendingTask {
                            is_blocking: true,
                            ok_action: self.ok_action,
                            rl_action: self.rl_action,
                            service: self.service,
                        },
                    )))
                } else {
                    Either::Left(None)
                }
            }
            PendingValue::Pending => Either::Right(self),
        }
    }
}

struct PendingTask {
    is_blocking: bool,
    ok_action: Option<AddResponseHeadersAction>,
    rl_action: Box<dyn Action>,
    service: Rc<Service>,
}

impl PendingTask {
    fn process_response(self, response: Vec<u8>) -> Option<Box<dyn Action>> {
        if self.service.parse_message(response) {
            Some(self.rl_action)
        } else if let Some(action) = self.ok_action {
            Some(Box::new(action))
        } else {
            None
        }
    }

    fn is_blocking(&self) -> bool {
        // This would need to peak into `ok_action` AND `rl_action` to see if we need to block
        self.is_blocking
    }
}

trait Action {
    fn apply(&self, ctx: &mut ReqRespCtx) -> bool;
}

struct AddResponseHeadersAction {
    headers: Vec<(String, String)>,
}

impl Action for AddResponseHeadersAction {
    fn apply(&self, ctx: &mut ReqRespCtx) -> bool {
        if *ctx.test_current_phase.borrow() == Some(ResponseHeaders) {
            ctx.response_headers = self.headers.clone();
            true
        } else {
            false
        }
    }
}

struct TooManyRequestsAction {}

impl Action for TooManyRequestsAction {
    fn apply(&self, ctx: &mut ReqRespCtx) -> bool {
        ctx.status_code = Some(429);
        true
    }
}

enum PendingValue<T> {
    Revolved(T),
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
    fn add_response_header(&mut self, key: &str, value: &str) {}
    fn next_token_id(&mut self) -> usize {
        self.test_token_id += 1;
        self.test_token_id
    }
}

mod tests {
    use super::*;

    #[test]
    fn it_rate_limits() {
        // on_request_headers() {
        let mut ctx = ReqRespCtx::default();
        ctx.test_predicate_values.push(PendingValue::Revolved(true));
        let mut pipeline = Pipeline {
            ctx,
            todos: vec![RLTask {
                predicate: Predicate {},
                service: Service {}.into(),
                ok_action: None,
                rl_action: Box::new(TooManyRequestsAction {}),
            }],
            pending_tasks: Default::default(),
            pending_actions: Default::default(),
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
        let mut rc = Rc::new(RefCell::new(Some(Phase::RequestHeaders)));
        ctx.test_current_phase = rc.clone();
        ctx.test_predicate_values.push(PendingValue::Revolved(true));
        let mut pipeline = Pipeline {
            ctx: ctx,
            todos: vec![RLTask {
                predicate: Predicate {},
                service: Service {}.into(),
                ok_action: Some(AddResponseHeadersAction {
                    headers: vec![("X-RateLimit-Limit".to_string(), "10".to_string())],
                }),
                rl_action: Box::new(TooManyRequestsAction {}),
            }],
            pending_tasks: Default::default(),
            pending_actions: Default::default(),
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
        let mut rc = Rc::new(RefCell::new(Some(Phase::RequestHeaders)));
        ctx.test_current_phase = rc.clone();
        ctx.test_predicate_values
            .insert(0, PendingValue::Revolved(true));
        ctx.test_predicate_values.insert(0, PendingValue::Pending);
        ctx.test_predicate_values.insert(0, PendingValue::Pending);
        ctx.test_predicate_values.insert(0, PendingValue::Pending);
        ctx.test_predicate_values
            .insert(0, PendingValue::Revolved(true));
        let mut pipeline = Pipeline {
            ctx: ctx,
            todos: vec![
                RLTask {
                    predicate: Predicate {},
                    service: Service {}.into(),
                    ok_action: None,
                    rl_action: Box::new(TooManyRequestsAction {}),
                },
                RLTask {
                    predicate: Predicate {},
                    service: Service {}.into(),
                    ok_action: None,
                    rl_action: Box::new(TooManyRequestsAction {}),
                },
            ],
            pending_tasks: Default::default(),
            pending_actions: Default::default(),
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
}

fn main() {}
