use axum::{
    extract::Query,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use bytes::Bytes;
use image::{io::Reader as ImageReader, ImageFormat};
use serde::Deserialize;
use sha1::{Digest, Sha1};
use std::{
    collections::HashMap,
    io::Cursor,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tower_http::cors::CorsLayer;

#[derive(Deserialize)]
struct ImageParams {
    quality: Option<u8>,
    width: Option<u32>,
    height: Option<u32>,
    format: Option<String>,
    image_url: String,
}

#[derive(Clone)]
struct ProcessedImageResult {
    data: Vec<u8>,
    content_type: String,
    original_width: u32,
    original_height: u32,
    etag: String,
}

#[derive(Clone)]
struct CacheEntry {
    result: ProcessedImageResult,
    size: usize,
    inserted: Instant,
}

type ImageCache = Arc<Mutex<HashMap<String, CacheEntry>>>;

// Настройки кэша
const CACHE_TTL: Duration = Duration::from_secs(3600); // 1 час
const CACHE_MAX_SIZE: usize = 150 * 1024 * 1024; // 150 МБ

#[tokio::main]
async fn main() {
    let cache: ImageCache = Arc::new(Mutex::new(HashMap::new()));
    let app = Router::new()
        .route(
            "/optimize",
            get({
                let cache = cache.clone();
                move |params| optimize_image(params, cache.clone())
            }),
        )
        .layer(CorsLayer::permissive());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3001").await.unwrap();

    println!("🚀 Rust Image Optimizer running on http://0.0.0.0:3001");

    axum::serve(listener, app).await.unwrap();
}

async fn optimize_image(
    Query(params): Query<ImageParams>,
    cache: ImageCache,
) -> Result<impl IntoResponse, StatusCode> {
    let cache_key = format!(
        "{}:{}:{}:{}:{}",
        params.image_url,
        params.width.unwrap_or(0),
        params.height.unwrap_or(0),
        params.quality.unwrap_or(80),
        params.format.clone().unwrap_or("jpeg".to_string())
    );

    // Проверка кэша
    if let Some(entry) = cache.lock().unwrap().get(&cache_key) {
        if entry.inserted.elapsed() < CACHE_TTL {
            let mut headers = HeaderMap::new();
            headers.insert("Content-Type", entry.result.content_type.parse().unwrap());
            headers.insert(
                "Cache-Control",
                HeaderValue::from_static("public, max-age=3600"),
            );
            headers.insert("ETag", entry.result.etag.parse().unwrap());
            return Ok((StatusCode::OK, headers, entry.result.data.clone()));
        }
    }

    // Получаем изображение
    let image_bytes = reqwest::get(&params.image_url)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?
        .bytes()
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    // Засекаем время начала обработки
    let start = Instant::now();
    let result = tokio::task::spawn_blocking(move || process_image(image_bytes, params))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let duration = start.elapsed();
    println!("⏱ Image processing took: {:.2?}", duration);

    // Генерация ETag
    let etag = format!("{:x}", Sha1::digest(&result.data));

    let result = ProcessedImageResult {
        etag: etag.clone(),
        ..result
    };

    // Добавляем в кэш
    {
        let mut cache_lock = cache.lock().unwrap();
        cache_lock.insert(
            cache_key,
            CacheEntry {
                size: result.data.len(),
                result: result.clone(),
                inserted: Instant::now(),
            },
        );
        enforce_cache_limit(&mut cache_lock);
    }

    // Ответ с headers
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", result.content_type.parse().unwrap());
    headers.insert(
        "Cache-Control",
        HeaderValue::from_static("public, max-age=3600"),
    );
    headers.insert("ETag", etag.parse().unwrap());

    Ok((StatusCode::OK, headers, result.data))
}

// Ограничение кэша по размеру
fn enforce_cache_limit(cache: &mut HashMap<String, CacheEntry>) {
    let mut total_size: usize = cache.values().map(|e| e.size).sum();
    if total_size <= CACHE_MAX_SIZE {
        return;
    }

    // Сортируем ключи по времени вставки (старые сначала)
    let mut keys: Vec<_> = cache.iter().map(|(k, v)| (k.clone(), v.inserted)).collect();
    keys.sort_by_key(|(_, inserted)| *inserted);

    for (key, _) in keys {
        if let Some(entry) = cache.remove(&key) {
            total_size -= entry.size;
        }
        if total_size <= CACHE_MAX_SIZE {
            break;
        }
    }
}

fn process_image(data: Bytes, params: ImageParams) -> Result<ProcessedImageResult, String> {
    let reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;

    let mut img = reader.decode().map_err(|e| e.to_string())?;
    let original_width = img.width();
    let original_height = img.height();

    // Ресайз изображения
    img = match (params.width, params.height) {
        (Some(w), Some(h)) => img.resize_exact(w, h, image::imageops::FilterType::Lanczos3),
        (Some(w), None) => img.resize(
            w,
            ((w as f32 / img.width() as f32) * img.height() as f32) as u32,
            image::imageops::FilterType::Lanczos3,
        ),
        (None, Some(h)) => img.resize(
            ((h as f32 / img.height() as f32) * img.width() as f32) as u32,
            h,
            image::imageops::FilterType::Lanczos3,
        ),
        _ => img,
    };

    let quality = params.quality.unwrap_or(80).clamp(1, 100);
    let format = params.format.as_deref().unwrap_or("jpeg");

    let mut output = Vec::new();
    let content_type = match format {
        "webp" => {
            let encoder = webp::Encoder::from_image(&img).map_err(|e| e.to_string())?;
            let webp_data = encoder.encode(quality as f32);
            output = webp_data.to_vec();
            "image/webp"
        }
        "png" => {
            img.write_to(&mut Cursor::new(&mut output), ImageFormat::Png)
                .map_err(|e| e.to_string())?;
            "image/png"
        }
        _ => {
            let mut jpeg_encoder =
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, quality);
            jpeg_encoder.encode_image(&img).map_err(|e| e.to_string())?;
            "image/jpeg"
        }
    };

    Ok(ProcessedImageResult {
        data: output,
        content_type: content_type.to_string(),
        original_width,
        original_height,
        etag: "".to_string(),
    })
}
