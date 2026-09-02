//! Framework-neutral control-plane authorization checkpoint.

mod middleware;

pub use middleware::{
    control_mutation_checkpoint, control_read_checkpoint, AuthorizedMutationContext,
    AuthorizedReadContext, ControlMiddlewareError, ControlRouteDescriptor, ControlRouteRegistry,
};
