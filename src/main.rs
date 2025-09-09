use std::io::Write;

struct Pipeline {
    task: RLTask,
}

impl Pipeline {
    fn eval(&self, ctx: &mut ReqRespCtx) {
        self.task.exec(ctx);
    }
}

struct RLTask {
    predicate: Predicate,
    service: String,
    ok_outcome: Action,
}

impl RLTask {
    fn exec(&self, ctx: &mut ReqRespCtx) {
        if self.predicate.eval() {
            call_grpc!("Call out to {}", self.service);
            // if ok then
            self.ok_outcome.apply(ctx);
            // otherwise
            // ko_outcome
        }
    }
}

struct Action {

}

impl Action {
    fn apply(&self, ctx: &mut ReqRespCtx) {
        ctx.add_response_header("Access-Control-Allow-Origin", "*");
    }
}

struct Predicate {

}

impl Predicate {
    fn eval(&self) -> bool{
        true
    }
}

struct ReqRespCtx {

}

impl ReqRespCtx {
    fn add_response_header(&mut self, key: &str, value: &str) {

    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use crate::{Action, Pipeline, Predicate, RLTask, ReqRespCtx};

    #[test]
    fn it_works() {
        let pipeline = Pipeline {
            task: RLTask {
                predicate: Predicate {},
                service: "limitador".to_string(),
                ok_outcome: Action {},
            }
        };

        // on_request_headers() {
        let ctx = &mut ReqRespCtx {};
        pipeline.eval();

        // on_request_body() {

        // on_response_headers() {

        // on_response_body() {
        pipeline.eval();


        // on_grpc_response() {
        pipeline.digest(response);

    }
}

fn main() {}