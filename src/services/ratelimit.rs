use crate::Service;
use crate::envoy::{RateLimitDescriptor, RateLimitRequest};
use protobuf::{Message, RepeatedField};
pub struct RateLimitService;

impl Service for RateLimitService {
    type Response = bool;
    fn dispatch(&self, ctx: &mut crate::ReqRespCtx) -> usize {
        // let msg = self.request_message(ctx.get_attribute::<String>("ratelimit.domain".into()));
        todo!()
    }

    fn parse_message(&self, message: Vec<u8>) -> bool {
        todo!()
    }
}

impl RateLimitService {
    fn request_message(
        domain: String,
        descriptors: RepeatedField<RateLimitDescriptor>,
        hits_addend: u32,
    ) -> RateLimitRequest {
        RateLimitRequest {
            domain,
            descriptors,
            hits_addend,
            unknown_fields: Default::default(),
            cached_size: Default::default(),
        }
    }
}
