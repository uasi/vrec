use std::collections::HashMap;
use std::path::PathBuf;

use actix_web::fs::NamedFile;
use actix_web::{
    error, http, server, App, Form, HttpRequest, HttpResponse, Responder, Result as AppResult,
};
use handlebars::{handlebars_helper, Handlebars};
use listenfd::ListenFd;
use mime_guess::guess_mime_type;
use serde_json::json;
use url::percent_encoding::{percent_decode, utf8_percent_encode, DEFAULT_ENCODE_SET};

use crate::recorder::{start_child_reaper, JobId, Recorder};

struct AppState {
    access_key: String,
    recorder: Recorder,
    handlebars: Handlebars,
}

pub fn start() -> std::io::Result<()> {
    dotenv::dotenv().ok();

    start_child_reaper();

    let mut listenfd = ListenFd::from_env();

    let mut server = server::new(move || {
        let access_key = std::env::var("ACCESS_KEY").expect("ACCESS_KEY must be set");

        let mut handlebars = Handlebars::new();
        handlebars.register_helper("encode", Box::new(percent_encode_helper));
        handlebars
            .register_templates_directory(".hbs", "./templates")
            .expect("Handlebars must initialize");

        let work_dir_path = dotenv::var("WORK_DIR").unwrap_or_else(|_| "var".to_owned());
        let work_dir_path = PathBuf::from(work_dir_path);

        let recorder = Recorder::new(work_dir_path);

        let app_state = AppState {
            access_key,
            recorder,
            handlebars,
        };

        App::with_state(app_state)
            // Doc: https://actix.rs/docs/url-dispatch/
            .resource("/", |r| r.get().f(get_index))
            .resource("/download", |r| {
                r.get().f(get_download);
                r.post().with(post_download);
            })
            .resource("/jobs/{id:[0-9A-Z]+}", |r| r.get().f(get_job))
            .resource("/jobs/{id:[0-9A-Z]+}/process", |r| {
                r.head().f(head_job_process)
            })
            .resource("/jobs/{id:[0-9A-Z]+}/{file_name:.*}", |r| {
                r.get().f(get_job_file)
            })
            .resource("/jobs", |r| r.get().f(get_jobs))
    });

    server = if let Some(listener) = listenfd.take_tcp_listener(0)? {
        server.listen(listener)
    } else {
        let port = dotenv::var("PORT").unwrap_or_else(|_| "3000".to_owned());
        let addr = format!("127.0.0.1:{}", port);
        println!("binding to {}", &addr);
        server.bind(addr)?
    };

    server.run();

    Ok(())
}

fn get_index(req: &HttpRequest<AppState>) -> AppResult<impl Responder> {
    render_html(&req.state().handlebars, "index", &())
}

fn get_download(req: &HttpRequest<AppState>) -> AppResult<impl Responder> {
    render_html(&req.state().handlebars, "download", &())
}

fn post_download(
    (req, params): (HttpRequest<AppState>, Form<Vec<(String, String)>>),
) -> impl Responder {
    let s = &req.state();

    let has_access_key = params
        .iter()
        .any(|(name, value)| name == "access_key" && value == &s.access_key);

    if !has_access_key {
        return HttpResponse::Unauthorized()
            .content_type("text/plain")
            .body("401 Unauthorized\n\nInvalid access key\n");
    }

    let args: Vec<&str> = params
        .iter()
        .filter_map(|(name, value)| {
            if name == "args[]" {
                let value = value.trim();
                if value != "" {
                    return Some(value);
                }
            }
            None
        })
        .collect();

    if args.is_empty() {
        return HttpResponse::Found()
            .header(http::header::LOCATION, "/download")
            .finish();
    }

    match s.recorder.spawn_job("youtube-dl", &args) {
        Ok(job) => HttpResponse::Found()
            .header(http::header::LOCATION, format!("/jobs/{}", job.id()))
            .finish(),
        Err(err) => HttpResponse::InternalServerError()
            .content_type("text/plain")
            .body(format!("500 Internal Server Error\n\n{:?}\n", err)),
    }
}

fn get_job(req: &HttpRequest<AppState>) -> AppResult<impl Responder> {
    fn sort_file_names(file_names: &mut Vec<String>) {
        fn key(file_name: &str) -> (u8, &str) {
            let order = match guess_mime_type(file_name).type_() {
                mime::VIDEO => 0,
                mime::AUDIO => 1,
                mime::IMAGE => 2,
                _ => 3,
            };
            (order, file_name)
        }

        file_names.sort_by(|a, b| key(&a).cmp(&key(&b)));
    }

    let s = &req.state();

    let job_id: JobId = From::<String>::from(req.match_info().query("id")?);

    let job = s
        .recorder
        .job(&job_id)
        .ok_or_else(|| error::ErrorNotFound(format!("Job {} not found", &job_id)))?;

    let invocation = job.invocation().unwrap_or_else(|| json!({}));

    let mut file_names = job.file_names();
    sort_file_names(&mut file_names);

    let mut h = HashMap::new();
    h.insert("id", json!(format!("{}", job_id)));
    h.insert("invocation", invocation);
    h.insert("file_names", json!(file_names));

    render_html(&s.handlebars, "job", &h)
}

fn head_job_process(req: &HttpRequest<AppState>) -> AppResult<impl Responder> {
    let s = &req.state();

    let job_id: JobId = From::<String>::from(req.match_info().query("id")?);
    let job = s.recorder.job(&job_id);

    if job.map(|j| j.is_running()).unwrap_or(false) {
        return Ok(HttpResponse::Ok().finish());
    }

    Ok(HttpResponse::NoContent().finish())
}

fn get_job_file(req: &HttpRequest<AppState>) -> AppResult<impl Responder> {
    let s = &req.state();

    let job_id: JobId = From::<String>::from(req.match_info().query("id")?);
    let job = s
        .recorder
        .job(&job_id)
        .ok_or_else(|| error::ErrorNotFound(""))?;

    // Documentation says query is percent-decoded automatically, but it seems it isn't.
    let file_name: String = req.match_info().query("file_name")?;
    let file_name = percent_decode(file_name.as_bytes())
        .decode_utf8_lossy()
        .to_string();

    let path = job.path().join(&file_name);
    let mut f = NamedFile::open(path)?;

    if file_name.ends_with(".txt") {
        f = f.set_content_type(mime::TEXT_PLAIN_UTF_8);
    }

    Ok(f)
}

fn get_jobs(req: &HttpRequest<AppState>) -> AppResult<impl Responder> {
    fn first_media_file_name(mut file_names: Vec<String>) -> Option<String> {
        file_names.sort();
        file_names.into_iter().find(|file_name| {
            let mime = guess_mime_type(&file_name);
            [mime::AUDIO, mime::VIDEO].contains(&mime.type_())
        })
    }

    let s = &req.state();

    let mut jobs: Vec<(String, Option<String>)> = s
        .recorder
        .jobs()
        .into_iter()
        .map(|job| {
            let id = job.id().to_string();
            let media_file_name = first_media_file_name(job.file_names());
            (id, media_file_name)
        })
        .collect();

    jobs.sort();
    jobs.reverse();

    render_html(&s.handlebars, "jobs", &jobs)
}

fn render_html<T>(handlebars: &Handlebars, template: &str, data: &T) -> AppResult<HttpResponse>
where
    T: serde::Serialize,
{
    match handlebars.render(template, data) {
        Ok(body) => Ok(HttpResponse::Ok().content_type("text/html").body(body)),
        Err(err) => Err(error::ErrorInternalServerError(err)),
    }
}

handlebars_helper!(percent_encode_helper: |s: str|
    utf8_percent_encode(s, DEFAULT_ENCODE_SET).to_string()
);
