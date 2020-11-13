use std::collections::HashMap;

use actix_files::NamedFile;
use actix_web::{error, http, web, HttpRequest, HttpResponse, Responder, Result as ActixResult};
use handlebars::Handlebars;
use percent_encoding::percent_decode;
use serde::Deserialize;
use serde_json::json;
use url::Url;

use crate::disk_stat::{humanize_byte_size, DiskStat};
use crate::recorder::{JobId, Recorder};
use crate::web::helpers::render_html;

type Data<'a> = web::Data<AppData<'a>>;

pub struct AppData<'a> {
    pub access_key: String,
    pub recorder: Recorder,
    pub handlebars: Handlebars<'a>,
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

pub fn configure_app(config: &mut web::ServiceConfig) {
    use web::{delete, get, head, post, resource as r};

    config
        .service(r("/").route(get().to(get_index)))
        .service(r("/api/record").route(post().to(post_api_record)))
        .service(
            r("/download")
                .route(get().to(get_download))
                .route(post().to(post_download)),
        )
        .service(r("/jobs/{id:[0-9A-Z]+}").route(get().to(get_job)))
        .service(r("/jobs/{id:[0-9A-Z]+}/process").route(head().to(head_job_process)))
        .service(r("/jobs/{id:[0-9A-Z]+}/{file_name:.*}").route(get().to(get_job_file)))
        .service(r("/jobs").route(get().to(get_jobs)))
        .service(r("/jobs").route(delete().to(delete_jobs)));
}

async fn post_api_record(
    data: Data<'_>,
    payload: web::Json<PostApiRecordPayload>,
) -> ActixResult<impl Responder> {
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

    println!("post_api_record {:?}", &payload);

    if payload.access_key != data.access_key {
        return Ok(HttpResponse::Unauthorized().finish());
    }

    if let Some(link) = extract_youtube_link(&payload.email_body) {
        println!("post_api_record link = {:?}", &link);
        data.recorder
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

async fn get_index(data: Data<'_>) -> ActixResult<impl Responder> {
    render_html(&data.handlebars, "index", &())
}

async fn get_download(data: Data<'_>) -> ActixResult<impl Responder> {
    render_html(&data.handlebars, "download", &())
}

async fn post_download(data: Data<'_>, params: web::Form<Vec<(String, String)>>) -> impl Responder {
    let has_access_key = params
        .iter()
        .any(|(name, value)| name == "access_key" && value == &data.access_key);

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

    match data.recorder.spawn_job("youtube-dl", &args) {
        Ok(job) => HttpResponse::Found()
            .header(http::header::LOCATION, format!("/jobs/{}", job.id()))
            .finish(),
        Err(err) => HttpResponse::InternalServerError()
            .content_type("text/plain")
            .body(format!("500 Internal Server Error\n\n{:?}\n", err)),
    }
}

async fn get_job(req: HttpRequest, data: Data<'_>) -> ActixResult<impl Responder> {
    fn sort_file_names(file_names: &mut Vec<String>) {
        fn key(file_name: &str) -> (u8, &str) {
            let mime = mime_guess::from_path(file_name).first_or_octet_stream();
            let order = match mime.type_() {
                mime::VIDEO => 0,
                mime::AUDIO => 1,
                mime::IMAGE => 2,
                _ => 3,
            };
            (order, file_name)
        }

        file_names.sort_by(|a, b| key(&a).cmp(&key(&b)));
    }

    let job_id: JobId = From::<String>::from(req.match_info().query("id").to_owned());

    let job = data
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

    render_html(&data.handlebars, "job", &h)
}

async fn head_job_process(req: HttpRequest, data: Data<'_>) -> ActixResult<impl Responder> {
    let job_id: JobId = From::<String>::from(req.match_info().query("id").to_owned());
    let job = data.recorder.job(&job_id);

    if job.map(|j| j.is_running()).unwrap_or(false) {
        return Ok(HttpResponse::Ok().finish());
    }

    Ok(HttpResponse::NoContent().finish())
}

async fn get_job_file(req: HttpRequest, data: Data<'_>) -> ActixResult<impl Responder> {
    let job_id: JobId = From::<String>::from(req.match_info().query("id").to_owned());
    let job = data
        .recorder
        .job(&job_id)
        .ok_or_else(|| error::ErrorNotFound(""))?;

    // Documentation says query is percent-decoded automatically, but it seems it isn't.
    let file_name: String = req.match_info().query("file_name").to_owned();
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

async fn get_jobs(data: Data<'_>) -> ActixResult<impl Responder> {
    fn first_media_file_name(mut file_names: Vec<String>) -> Option<String> {
        file_names.sort();
        file_names.into_iter().find(|file_name| {
            let mime = mime_guess::from_path(&file_name).first_or_octet_stream();
            [mime::AUDIO, mime::VIDEO].contains(&mime.type_())
        })
    }

    let mut jobs: Vec<(String, Option<String>)> = data
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
    if let Some(stat) = DiskStat::new(data.recorder.work_dir_path()) {
        h.insert("disk_available", json!(humanize_byte_size(stat.available)));
        h.insert("disk_total", json!(humanize_byte_size(stat.total)));
        h.insert("disk_used", json!(humanize_byte_size(stat.used)));
    } else {
        h.insert("disk_available", json!("N/A"));
        h.insert("disk_total", json!("N/A"));
        h.insert("disk_used", json!("N/A"));
    }

    render_html(&data.handlebars, "jobs", &h)
}

async fn delete_jobs(
    data: Data<'_>,
    payload: web::Json<DeleteJobsPayload>,
) -> ActixResult<impl Responder> {
    println!("delete_jobs {:?}", &payload);

    if payload.access_key != data.access_key {
        return Ok(HttpResponse::Unauthorized().finish());
    }

    for job_id in &payload.job_ids {
        if let Some(job) = data.recorder.job(&job_id.clone().into()) {
            job.safe_delete();
        }
    }

    Ok(HttpResponse::Ok().finish())
}
