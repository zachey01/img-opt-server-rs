use actix_multipart::Multipart;
use actix_web::{web, App, HttpResponse, HttpServer, Responder};
use futures_util::StreamExt;
use std::io::Cursor;

use image::{io::Reader as ImageReader, DynamicImage, ImageBuffer, ImageOutputFormat, Rgba};

use fast_image_resize as fir;
use fast_image_resize::images::Image;
use fast_image_resize::Resizer;

async fn resize_image(mut payload: Multipart) -> impl Responder {
    while let Some(item) = payload.next().await {
        if let Ok(mut field) = item {
            let mut data = Vec::new();
            while let Some(chunk) = field.next().await {
                let chunk = chunk.unwrap();
                data.extend_from_slice(&chunk);
            }

            // Загружаем изображение с определением формата
            let img_reader = ImageReader::new(Cursor::new(&data))
                .with_guessed_format()
                .unwrap();
            let format = img_reader.format().unwrap(); // сохраняем формат (JPEG, PNG и т.д.)
            let img = img_reader.decode().unwrap().to_rgba8();

            let (width, height) = img.dimensions();

            // Создаем Image для fast_image_resize
            let mut src_image = Image::new(width, height, fir::PixelType::U8x4);
            src_image.buffer_mut().copy_from_slice(&img.into_raw());

            // Целевой размер
            let dst_width: u32 = 200;
            let dst_height: u32 = 200;
            let mut dst_image = Image::new(dst_width, dst_height, fir::PixelType::U8x4);

            // Ресайз
            let mut resizer = Resizer::new();
            resizer.resize(&src_image, &mut dst_image, None).unwrap();

            // Конвертируем обратно в DynamicImage
            let buffer = dst_image.buffer();
            let img_buffer =
                ImageBuffer::<Rgba<u8>, _>::from_raw(dst_width, dst_height, buffer.to_vec())
                    .unwrap();
            let dyn_image = DynamicImage::ImageRgba8(img_buffer);

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
                        .write_to(&mut Cursor::new(&mut bytes), ImageOutputFormat::Jpeg(80))
                        .unwrap();
                    "image/jpeg"
                }
                image::ImageFormat::Gif => {
                    dyn_image
                        .write_to(&mut Cursor::new(&mut bytes), ImageOutputFormat::Gif)
                        .unwrap();
                    "image/gif"
                }
                image::ImageFormat::Bmp => {
                    dyn_image
                        .write_to(&mut Cursor::new(&mut bytes), ImageOutputFormat::Bmp)
                        .unwrap();
                    "image/bmp"
                }
                image::ImageFormat::Tiff => {
                    dyn_image
                        .write_to(&mut Cursor::new(&mut bytes), ImageOutputFormat::Tiff)
                        .unwrap();
                    "image/tiff"
                }
                _ => {
                    // если формат неизвестен, используем PNG как fallback
                    dyn_image
                        .write_to(&mut Cursor::new(&mut bytes), ImageOutputFormat::Png)
                        .unwrap();
                    "image/png"
                }
            };

            return HttpResponse::Ok().content_type(content_type).body(bytes);
        }
    }

    HttpResponse::BadRequest().body("No file uploaded")
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    HttpServer::new(|| App::new().route("/resize", web::post().to(resize_image)))
        .bind("127.0.0.1:8080")?
        .run()
        .await
}
