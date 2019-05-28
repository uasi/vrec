use std::path::PathBuf;

use actix_web::{App, HttpServer};
use handlebars::Handlebars;
use listenfd::ListenFd;

use crate::recorder::{start_child_reaper, Recorder};
use crate::web::services::{configure_app, AppData};

mod helpers;
mod services;

pub fn start() -> std::io::Result<()> {
    dotenv::dotenv().ok();

    start_child_reaper();

    let mut listenfd = ListenFd::from_env();

    let mut server = HttpServer::new(move || {
        let access_key = std::env::var("ACCESS_KEY").expect("ACCESS_KEY must be set");

        let mut handlebars = Handlebars::new();
        helpers::register_handlebars_helpers(&mut handlebars);
        handlebars
            .register_templates_directory(".hbs", "./templates")
            .expect("Handlebars must initialize");

        let var_dir_path = dotenv::var("VAR_DIR").unwrap_or_else(|_| "var".to_owned());
        let recorder_dir_path = PathBuf::from(var_dir_path).join("jobs");

        let recorder = Recorder::new(recorder_dir_path);

        let data = AppData {
            access_key,
            recorder,
            handlebars,
        };

        App::new().data(data).configure(configure_app)
    });

    server = if let Some(listener) = listenfd.take_tcp_listener(0)? {
        server.listen(listener)?
    } else {
        let port = dotenv::var("PORT").unwrap_or_else(|_| "3000".to_owned());
        let addr = format!("127.0.0.1:{}", port);
        println!("binding to {}", &addr);
        server.bind(addr)?
    };

    server.run()
}
