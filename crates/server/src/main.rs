use server::config::Config;

#[tokio::main]
async fn main() {
    env_logger::init();

    let config = std::fs::read_to_string("./config.toml").expect("Config is not provided");
    let config = toml::from_str::<Config>(&config).expect("Invalid config");

    server::start_server(config).await;
}
