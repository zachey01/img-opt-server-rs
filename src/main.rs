use actix_multipart::Multipart;
use actix_web::{web, App, HttpResponse, HttpServer, Responder};
use fast_image_resize as fir;
use fast_image_resize::images::Image;
use fast_image_resize::Resizer;
use futures_util::StreamExt;
use image::{io::Reader as ImageReader, DynamicImage, ImageBuffer, ImageOutputFormat, Rgba};
use serde::Deserialize;
use std::io::Cursor;

#[derive(Debug, Deserialize)]
struct ResizeParams {
    width: u32,
    height: u32,
    #[serde(default = "default_quality")]
    quality: u8,
    url: Option<String>,
}

fn default_quality() -> u8 {
    80
}

async fn resize_image(
    query: web::Query<ResizeParams>,
    mut payload: Option<Multipart>,
) -> impl Responder {
    let mut img_data: Vec<u8> = Vec::new();

    // Если передан URL
    if let Some(url) = &query.url {
        match reqwest::get(url).await {
            Ok(resp) => match resp.bytes().await {
                Ok(bytes) => img_data.extend_from_slice(&bytes),
                Err(_) => {
                    return HttpResponse::BadRequest().body("Failed to read image bytes from URL")
                }
            },
            Err(_) => return HttpResponse::BadRequest().body("Failed to fetch image from URL"),
        }
    }
    // Иначе ожидаем multipart загрузку
    else if let Some(payload) = payload.as_mut() {
        while let Some(item) = payload.next().await {
            let mut field = match item {
                Ok(f) => f,
                Err(_) => continue,
            };

            while let Some(chunk) = field.next().await {
                match chunk {
                    Ok(bytes) => img_data.extend_from_slice(&bytes),
                    Err(_) => return HttpResponse::BadRequest().body("Error reading file chunk"),
                }
            }
        }
    } else {
        return HttpResponse::BadRequest().body("No image provided");
    }

    // Загружаем изображение
    let img_reader = ImageReader::new(Cursor::new(&img_data))
        .with_guessed_format()
        .unwrap();
    let format = img_reader.format().unwrap_or(image::ImageFormat::Png);
    let img = img_reader.decode().unwrap().to_rgba8();

    let (width_orig, height_orig) = img.dimensions();

    // Создаем Image для fast_image_resize
    let mut src_image = Image::new(width_orig, height_orig, fir::PixelType::U8x4);
    src_image.buffer_mut().copy_from_slice(&img.into_raw());

    // Целевой размер
    let dst_width = query.width;
    let dst_height = query.height;
    let mut dst_image = Image::new(dst_width, dst_height, fir::PixelType::U8x4);

    // Ресайз
    let mut resizer = Resizer::new();
    resizer.resize(&src_image, &mut dst_image, None).unwrap();

    // Конвертируем обратно в DynamicImage
    let buffer = dst_image.buffer();
    let img_buffer =
        ImageBuffer::<Rgba<u8>, _>::from_raw(dst_width, dst_height, buffer.to_vec()).unwrap();
    let dyn_image = DynamicImage::ImageRgba8(img_buffer);

    // Конвертируем в bytes с нужным форматом
    let mut bytes = Vec::new();
    let content_type = match format {
        image::ImageFormat::Png => {
            dyn_image
                .write_to(&mut Cursor::new(&mut bytes), ImageOutputFormat::Png)
                .unwrap();
            "image/png"
        }
        image::ImageFormat::Jpeg => {
            dyn_image
                .write_to(
                    &mut Cursor::new(&mut bytes),
                    ImageOutputFormat::Jpeg(query.quality),
                )
                .unwrap();
            "image/jpeg"
        }
        _ => {
            dyn_image
                .write_to(&mut Cursor::new(&mut bytes), ImageOutputFormat::Png)
                .unwrap();
            "image/png"
        }
    };

    HttpResponse::Ok().content_type(content_type).body(bytes)
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    HttpServer::new(|| {
        App::new()
            .route("/resize", web::post().to(resize_image))
            .route("/resize", web::get().to(resize_image)) // поддержка GET для URL
    })
    .bind("127.0.0.1:3001")?
    .run()
    .await
}
