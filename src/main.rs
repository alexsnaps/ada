use std::{collections::BTreeMap, ops::Not, rc::Rc};

struct Service {}

impl Service {
    pub(crate) fn dispatch(&self, ctx: &mut ReqRespCtx) -> usize {
        0
    }
    fn parse_message(&self, mut message: Vec<u8>) -> bool {
        Some(1u8) == message.pop()
    }
}

struct Pipeline {
    ctx: ReqRespCtx,
    todos: Vec<RLTask>,
    pendings: BTreeMap<usize, PendingTask>,
}

impl Pipeline {
    fn eval(mut self) -> Option<Self> {
        for todo in self.todos.drain(..) {
            if let Some((token_id, t)) = todo.exec(&mut self.ctx) {
                self.pendings.insert(token_id, t);
            }
        }
        if self.pendings.is_empty() {
            None
        } else {
            Some(self)
        }
    }

    fn digest(&mut self, token_id: usize, response: Vec<u8>) {
        if let Some(pending) = self.pendings.remove(&token_id) {
            // Process the response
            if let Some(action) = pending.process_response(response) {
                action.apply(&mut self.ctx);
            };
        }
    }

    fn is_blocked(&self) -> bool {
        self.pendings.values().any(PendingTask::is_blocking)
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        if self.todos.is_empty().not() || self.pendings.is_empty().not() {
            panic!("Pipeline dropped with pending tasks");
        }
    }
}

struct RLTask {
    predicate: Predicate,
    service: Rc<Service>,
    ok_action: Option<AddResponseHeadersAction>,
    rl_action: TooManyRequestsAction,
}

impl RLTask {
    fn exec(self, ctx: &mut ReqRespCtx) -> Option<(usize, PendingTask)> {
        if self.predicate.eval() {
            let token_id: usize = self.service.dispatch(ctx);
            return Some((
                token_id,
                PendingTask {
                    is_blocking: true,
                    ok_action: self.ok_action,
                    rl_action: self.rl_action,
                    service: self.service,
                },
            ));
        }
        None
    }
}

struct PendingTask {
    is_blocking: bool,
    ok_action: Option<AddResponseHeadersAction>,
    rl_action: TooManyRequestsAction,
    service: Rc<Service>,
}

impl PendingTask {
    fn process_response(self, response: Vec<u8>) -> Option<Box<dyn Action>> {
        if self.service.parse_message(response) {
            Some(Box::new(self.rl_action))
        } else if let Some(action) = self.ok_action {
            Some(Box::new(action))
        } else {
            None
        }
    }

    fn is_blocking(&self) -> bool {
        self.is_blocking
    }
}

trait Action {
    fn apply(&self, ctx: &mut ReqRespCtx);
}

struct AddResponseHeadersAction {
    headers: Vec<(String, String)>,
}

impl Action for AddResponseHeadersAction {
    fn apply(&self, ctx: &mut ReqRespCtx) {
        ctx.response_headers = self.headers.clone();
    }
}

struct TooManyRequestsAction {}

impl Action for TooManyRequestsAction {
    fn apply(&self, ctx: &mut ReqRespCtx) {
        ctx.status_code = Some(429);
    }
}

struct Predicate {}

impl Predicate {
    fn eval(&self) -> bool {
        true
    }
}

#[derive(Default)]
struct ReqRespCtx {
    status_code: Option<u32>,
    response_headers: Vec<(String, String)>,
}

impl ReqRespCtx {
    fn add_response_header(&mut self, key: &str, value: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_rate_limits() {
        // on_request_headers() {
        let mut pipeline = Pipeline {
            ctx: Default::default(),
            todos: vec![RLTask {
                predicate: Predicate {},
                service: Service {}.into(),
                ok_action: None,
                rl_action: TooManyRequestsAction {},
            }],
            pendings: Default::default(),
        };

        pipeline = pipeline
            .eval()
            .expect("Pipeline should be waiting for limitador");
        assert!(pipeline.is_blocked(), "Filter should be paused");

        // fn on_grpc_call_response(&mut self, token_id: u32, status_code: u32, resp_size: usize) {
        let buffer: Vec<u8> = vec![1u8];
        let token_id = 0;
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
        let mut pipeline = Pipeline {
            ctx: Default::default(),
            todos: vec![RLTask {
                predicate: Predicate {},
                service: Service {}.into(),
                ok_action: Some(AddResponseHeadersAction {
                    headers: vec![("X-RateLimit-Limit".to_string(), "10".to_string())],
                }),
                rl_action: TooManyRequestsAction {},
            }],
            pendings: Default::default(),
        };

        pipeline = pipeline
            .eval()
            .expect("Pipeline should be waiting for limitador");
        assert!(pipeline.is_blocked(), "Filter should be paused");

        // fn on_grpc_call_response(&mut self, token_id: u32, status_code: u32, resp_size: usize) {
        let buffer: Vec<u8> = Vec::new();
        let token_id = 0;
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
        assert!(pipeline.eval().is_none(), "Done now");
        // assert_eq!(
        //     pipeline.ctx.headers,
        //     vec![("x".to_string(), "y".to_string())]
        // );

        // on_response_body() {
        // pipeline.eval();
    }
}

fn main() {}
