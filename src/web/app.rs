use std::collections::HashMap;

use actix_web::fs::NamedFile;
use actix_web::{
    error, http, App, Form, HttpRequest, HttpResponse, Json, Responder, Result as AppResult,
};
use handlebars::Handlebars;
use mime_guess::guess_mime_type;
use serde::Deserialize;
use serde_json::json;
use url::{percent_encoding::percent_decode, Url};

use crate::disk_stat::{humanize_byte_size, DiskStat};
use crate::recorder::{JobId, Recorder};
use crate::web::helpers::render_html;

pub struct AppState {
    pub access_key: String,
    pub recorder: Recorder,
    pub handlebars: Handlebars,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PostApiRecordPayload {
    access_key: String,
    email_subject: String,
    email_body: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteJobsPayload {
    access_key: String,
    job_ids: Vec<String>,
}

pub fn new(app_state: AppState) -> App<AppState> {
    App::with_state(app_state)
        // Doc: https://actix.rs/docs/url-dispatch/
        .resource("/", |r| r.get().f(get_index))
        .resource("/api/record", |r| r.post().with(post_api_record))
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
        .resource("/jobs", |r| r.delete().with(delete_jobs))
}

fn post_api_record(
    (req, payload): (HttpRequest<AppState>, Json<PostApiRecordPayload>),
) -> AppResult<impl Responder> {
    fn find_youtube_link(link: linkify::Link) -> Option<String> {
        Url::parse(link.as_str())
            .into_iter()
            .find(|url| url.domain() == Some("www.youtube.com") && url.path() == "/watch")
            .map(Url::into_string)
    }

    fn extract_youtube_link(text: &str) -> Option<String> {
        let mut finder = linkify::LinkFinder::new();
        finder.kinds(&[linkify::LinkKind::Url]);
        finder.links(text).filter_map(find_youtube_link).next()
    }

    let s = req.state();

    println!("post_api_record {:?}", &payload);

    if payload.access_key != s.access_key {
        return Ok(HttpResponse::Unauthorized().finish());
    }

    if let Some(link) = extract_youtube_link(&payload.email_body) {
        println!("post_api_record link = {:?}", &link);
        s.recorder
            .spawn_job(
                "youtube-dl",
                &["--write-all-thumbnails", "--write-info-json", link.as_str()],
            )
            .and_then(|_| Ok(Ok(HttpResponse::Created().finish())))
            .unwrap_or_else(|_| Ok(HttpResponse::Ok().finish()))
    } else {
        println!("post_api_record link not found");
        Ok(HttpResponse::Ok().finish())
    }
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

    let mut h = HashMap::new();
    h.insert("jobs", json!(jobs));
    if let Some(stat) = DiskStat::new(req.state().recorder.work_dir_path()) {
        h.insert("disk_available", json!(humanize_byte_size(stat.available)));
        h.insert("disk_total", json!(humanize_byte_size(stat.total)));
        h.insert("disk_used", json!(humanize_byte_size(stat.used)));
    } else {
        h.insert("disk_available", json!("N/A"));
        h.insert("disk_total", json!("N/A"));
        h.insert("disk_used", json!("N/A"));
    }

    render_html(&s.handlebars, "jobs", &h)
}

fn delete_jobs(
    (req, payload): (HttpRequest<AppState>, Json<DeleteJobsPayload>),
) -> AppResult<impl Responder> {
    let s = req.state();

    println!("delete_jobs {:?}", &payload);

    if payload.access_key != s.access_key {
        return Ok(HttpResponse::Unauthorized().finish());
    }

    for job_id in &payload.job_ids {
        if let Some(job) = s.recorder.job(&job_id.clone().into()) {
            job.safe_delete();
        }
    }

    Ok(HttpResponse::Ok().finish())
}
