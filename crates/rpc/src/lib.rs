pub mod common;

pub mod models;

pub mod client;
pub mod server;

#[macro_export]
macro_rules! register_endpoints {
    ($router:expr, $($endpoint:ident),+ $(,)?) => {
        $router
            $(
                .register(
                    $endpoint::key(),
                    $endpoint::build
                )
            )+
    };
}
