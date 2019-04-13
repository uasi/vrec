use actix_web::{error, HttpResponse, Result as AppResult};
use handlebars::Handlebars;

pub fn render_html<T>(handlebars: &Handlebars, template: &str, data: &T) -> AppResult<HttpResponse>
where
    T: serde::Serialize,
{
    match handlebars.render(template, data) {
        Ok(body) => Ok(HttpResponse::Ok().content_type("text/html").body(body)),
        Err(err) => Err(error::ErrorInternalServerError(err)),
    }
}

pub fn register_handlebars_helpers(handlebars: &mut Handlebars) {
    use self::handlebars_helpers::*;

    handlebars.register_helper("encode", Box::new(percent_encode_helper));
    handlebars.register_helper(
        "datetime_from_job_id",
        Box::new(datetime_from_job_id_helper),
    );
}

#[allow(clippy::redundant_closure)]
mod handlebars_helpers {
    use handlebars::handlebars_helper;
    use url::percent_encoding::{utf8_percent_encode, DEFAULT_ENCODE_SET};

    handlebars_helper!(datetime_from_job_id_helper: |s: str|
        ulid::Ulid::from_string(s)
            .map(|ulid| ulid.datetime().to_rfc3339())
            .unwrap_or_default()
    );

    handlebars_helper!(percent_encode_helper: |s: str|
        utf8_percent_encode(s, DEFAULT_ENCODE_SET).to_string()
    );
}
