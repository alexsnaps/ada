use std::rc::Rc;

struct PendingTask {
    call_id: usize,
}

struct Pipeline {
    ctx: ReqRespCtx,
    todos: Vec<RLTask>,
    pendings: Vec<PendingTask>,
}

impl Pipeline {
    fn eval(&mut self) {
        for todo in self.todos.drain(..) {
            if let Some(t) = todo.exec(&mut self.ctx) {
                self.pendings.push(t);
            }
        }
    }
}

struct RLTask {
    predicate: Predicate,
    service: Rc<Service>,
    ok_outcome: Action,
}

impl RLTask {
    fn exec(self, ctx: &mut ReqRespCtx) -> Option<PendingTask> {
        if self.predicate.eval() {
            let call_id: usize = self.service.dispatch(ctx);
            return Some(PendingTask { call_id });
        }
        None
    }
}

enum Task {
    Todo(RLTask),
    Pending(()),
}

struct Service {}

impl Service {
    pub(crate) fn dispatch(&self, ctx: &mut ReqRespCtx) -> usize {
        0
    }
}

struct Action {}

impl Action {
    fn apply(&self, ctx: &mut ReqRespCtx) {
        ctx.add_response_header("Access-Control-Allow-Origin", "*");
    }
}

struct Predicate {}

impl Predicate {
    fn eval(&self) -> bool {
        true
    }
}

struct ReqRespCtx {}

impl ReqRespCtx {
    fn add_response_header(&mut self, key: &str, value: &str) {}
}

#[cfg(test)]
mod tests {
    use crate::{Action, Pipeline, Predicate, RLTask, ReqRespCtx};
    use Service;

    #[test]
    fn it_works() {
        // on_request_headers() {
        let ctx = ReqRespCtx {};
        let mut pipeline = Pipeline {
            ctx,
            todos: vec![RLTask {
                predicate: Predicate {},
                service: Service {}.into(),
                ok_outcome: Action {},
            }],
            pendings: vec![],
        };

        pipeline.eval();

        // on_grpc_response() {
        //pipeline.digest(response);

        // on_request_body() {

        // on_response_headers() {

        // on_response_body() {
        pipeline.eval();
    }
}

fn main() {}
