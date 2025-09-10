use crate::PendingValue::Resolved;
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
    todos: Vec<Box<dyn Action>>,
    pending_tasks: BTreeMap<usize, PendingTask>,
    pending_actions: Vec<Box<dyn Action>>,
}

impl Pipeline {
    fn eval(mut self) -> Option<Self> {
        let mut actions = Vec::default();
        for action in self.pending_actions.drain(..) {
            match action.apply(&mut self.ctx) {
                ActionOutcome::Done => (),
                ActionOutcome::Deferred(_) => panic!("blah"),
                ActionOutcome::Pending(action) => actions.push(action),
            };
        }
        self.pending_actions = actions;

        let mut todos = Vec::default();
        for task in self.todos.drain(..) {
            match task.apply(&mut self.ctx) {
                ActionOutcome::Done => {}
                ActionOutcome::Deferred((token_id, t)) => {
                    if self.pending_tasks.contains_key(&token_id) {
                        panic!("NONONONONO")
                    }
                    self.pending_tasks.insert(token_id, t);
                }
                ActionOutcome::Pending(action) => todos.push(action),
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
            if let Some(action) = pending.process_response(response) {
                match action.apply(&mut self.ctx) {
                    ActionOutcome::Done => {}
                    ActionOutcome::Deferred(_) => panic!("Action should not be deferred in digest"),
                    ActionOutcome::Pending(action) => self.pending_actions.push(action),
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

impl Action for RLTask {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> ActionOutcome {
        match self.predicate.eval(ctx) {
            Resolved(exec) => {
                if exec {
                    let token_id: usize = self.service.dispatch(ctx);
                    ActionOutcome::Deferred((
                        token_id,
                        PendingTask {
                            is_blocking: true,
                            ok_action: self.ok_action,
                            rl_action: self.rl_action,
                            service: self.service,
                        },
                    ))
                } else {
                    ActionOutcome::Done
                }
            }
            PendingValue::Pending => ActionOutcome::Pending(Box::new(RLTask {
                predicate: self.predicate,
                ok_action: self.ok_action,
                rl_action: self.rl_action,
                service: self.service,
            })),
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

enum ActionOutcome {
    Done,
    Deferred((usize, PendingTask)),
    Pending(Box<dyn Action>),
}

trait Action {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> ActionOutcome;
}

#[derive(Clone)]
struct AddResponseHeadersAction {
    headers: Vec<(String, String)>,
}

impl Action for AddResponseHeadersAction {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> ActionOutcome {
        if *ctx.test_current_phase.borrow() == Some(ResponseHeaders) {
            ctx.response_headers = self.headers.clone();
            ActionOutcome::Done
        } else {
            ActionOutcome::Pending(Box::new(AddResponseHeadersAction {
                headers: self.headers,
            }))
        }
    }
}

struct TooManyRequestsAction {}

impl Action for TooManyRequestsAction {
    fn apply(self: Box<Self>, ctx: &mut ReqRespCtx) -> ActionOutcome {
        ctx.status_code = Some(429);
        ActionOutcome::Done
    }
}

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
        ctx.test_predicate_values.push(PendingValue::Resolved(true));
        let mut pipeline = Pipeline {
            ctx,
            todos: vec![Box::new(RLTask {
                predicate: Predicate {},
                service: Service {}.into(),
                ok_action: None,
                rl_action: Box::new(TooManyRequestsAction {}),
            })],
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
        ctx.test_predicate_values.push(PendingValue::Resolved(true));
        let mut pipeline = Pipeline {
            ctx: ctx,
            todos: vec![Box::new(RLTask {
                predicate: Predicate {},
                service: Service {}.into(),
                ok_action: Some(AddResponseHeadersAction {
                    headers: vec![("X-RateLimit-Limit".to_string(), "10".to_string())],
                }),
                rl_action: Box::new(TooManyRequestsAction {}),
            })],
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
                    service: Service {}.into(),
                    ok_action: None,
                    rl_action: Box::new(TooManyRequestsAction {}),
                }),
                Box::new(RLTask {
                    predicate: Predicate {},
                    service: Service {}.into(),
                    ok_action: None,
                    rl_action: Box::new(TooManyRequestsAction {}),
                }),
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
