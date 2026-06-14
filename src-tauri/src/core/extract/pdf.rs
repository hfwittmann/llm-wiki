//! PDF image extraction via pdfium.
//!
//! All functions here hold the global pdfium lock for their full duration.
//! Callers MUST NOT acquire the lock themselves before calling (non-reentrant).

use std::path::Path;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};

use super::{save_one_image, sha256_hex, ExtractError, ExtractOptions, ExtractedImage, SavedImage};

/// Combined PDF text + image extraction in a single pdfium session.
///
/// Output is a markdown string with `## Page N` headers, the page's
/// extracted text, and `![](url)` references to images embedded on
/// that page — interleaved per-page so the document reads top-to-bottom
/// the way the source did.
///
/// When `media_dest_dir` is `Some`, every embedded raster image passing the
/// size filter is written to that directory as `img-<N>.png` and referenced
/// in the markdown via `media_url_prefix + "/img-<N>.png"`.
///
/// When `media_dest_dir` is `None`, image objects are skipped entirely.
///
/// Holds the global pdfium lock for its full duration. Callers MUST NOT
/// acquire the lock themselves before calling this (would deadlock).
pub fn extract_markdown(
    path: &str,
    media_dest_dir: Option<&Path>,
    media_url_prefix: &str,
    options: &ExtractOptions,
) -> Result<String, ExtractError> {
    use pdfium_render::prelude::*;

    let _guard = crate::core::files::lock_pdfium();
    let pdfium = crate::core::files::pdfium().map_err(|e| ExtractError::Pdfium(e))?;
    let doc = pdfium.load_pdf_from_file(path, None).map_err(|e| match e {
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
            ExtractError::Pdfium(format!(
                "PDF is password-protected and cannot be read: '{path}'"
            ))
        }
        _ => ExtractError::Pdfium(format!("Failed to open PDF '{path}': {e}")),
    })?;

    let mut out = String::new();
    let mut idx: u32 = 0;
    let mut total_saved: u32 = 0;
    // Strip a single trailing slash from the prefix so we can always
    // emit `prefix + "/" + name` without producing `path//name`.
    let prefix = media_url_prefix.trim_end_matches('/');

    let page_count = doc.pages().len();
    if media_dest_dir.is_some() {
        eprintln!(
            "[extract_pdf_markdown] '{path}': {page_count} page(s), images→{:?}",
            media_dest_dir.map(|d| d.display().to_string())
        );
    }

    for (page_idx, page) in doc.pages().iter().enumerate() {
        let page_num = page_idx + 1;
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&format!("## Page {page_num}\n\n"));

        let page_text = page.text().map_err(|e| {
            ExtractError::Pdfium(format!(
                "Page {page_num} text extraction failed in '{path}': {e}"
            ))
        })?;
        out.push_str(&page_text.all());
        out.push('\n');

        let dest_dir = match media_dest_dir {
            Some(d) => d,
            None => continue,
        };

        let mut page_image_md: Vec<String> = Vec::new();
        for object in page.objects().iter() {
            let image = match object.as_image_object() {
                Some(img) => img,
                None => continue,
            };
            let dyn_img = match image.get_raw_image() {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("[extract_pdf_markdown] page {page_num} image read failed: {e}");
                    continue;
                }
            };
            let width = dyn_img.width();
            let height = dyn_img.height();
            if width < options.min_width || height < options.min_height {
                continue;
            }
            let mut png_bytes: Vec<u8> = Vec::new();
            if let Err(e) = dyn_img.write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            ) {
                eprintln!("[extract_pdf_markdown] page {page_num} PNG encode failed: {e}");
                continue;
            }
            idx += 1;
            let file_name = format!("img-{idx}.png");
            if let Err(e) = save_one_image(&png_bytes, dest_dir, dest_dir, &file_name) {
                eprintln!("[extract_pdf_markdown] page {page_num} save failed: {e}");
                continue;
            }
            total_saved += 1;
            page_image_md.push(format!("![]({prefix}/{file_name})"));
            if total_saved as usize >= options.max_images {
                eprintln!(
                    "[extract_pdf_markdown] reached max_images={} cap; skipped rest",
                    options.max_images
                );
                break;
            }
        }
        if !page_image_md.is_empty() {
            out.push('\n');
            for img_md in &page_image_md {
                out.push_str(img_md);
                out.push('\n');
            }
        }
        if total_saved as usize >= options.max_images {
            break;
        }
    }

    if media_dest_dir.is_some() {
        eprintln!("[extract_pdf_markdown] '{path}' DONE — pages={page_count}, saved={total_saved}");
    }

    Ok(out)
}

/// Iterate every PDF page, extract every embedded raster image, and
/// re-encode each to PNG. Vector content (paths, glyph outlines) is NOT
/// extracted here.
pub fn extract_images(path: &str, options: &ExtractOptions) -> Result<Vec<ExtractedImage>, ExtractError> {
    use pdfium_render::prelude::*;

    let _guard = crate::core::files::lock_pdfium();
    let pdfium = crate::core::files::pdfium().map_err(|e| ExtractError::Pdfium(e))?;
    let doc = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|e| ExtractError::Pdfium(format!("Failed to open PDF '{path}': {e}")))?;

    let mut out: Vec<ExtractedImage> = Vec::new();
    let mut idx: u32 = 0;

    'pages: for (page_idx, page) in doc.pages().iter().enumerate() {
        for object in page.objects().iter() {
            let image = match object.as_image_object() {
                Some(img) => img,
                None => continue,
            };

            let dyn_img = match image.get_raw_image() {
                Ok(b) => b,
                Err(e) => {
                    eprintln!(
                        "[extract_pdf_images] page {} image read failed: {e}",
                        page_idx + 1
                    );
                    continue;
                }
            };

            let width = dyn_img.width();
            let height = dyn_img.height();
            if width < options.min_width || height < options.min_height {
                continue;
            }

            let mut png_bytes: Vec<u8> = Vec::new();
            if let Err(e) = dyn_img.write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            ) {
                eprintln!(
                    "[extract_pdf_images] page {} PNG encode failed: {e}",
                    page_idx + 1
                );
                continue;
            }

            idx += 1;
            let data_base64 = B64.encode(&png_bytes);
            let sha256 = sha256_hex(&png_bytes);

            out.push(ExtractedImage {
                index: idx,
                mime_type: "image/png".to_string(),
                page: Some((page_idx + 1) as u32),
                width,
                height,
                data_base64,
                sha256,
            });

            if out.len() >= options.max_images {
                eprintln!(
                    "[extract_pdf_images] reached max_images={} cap; remaining images skipped",
                    options.max_images
                );
                break 'pages;
            }
        }
    }

    Ok(out)
}

/// PDF: extract every embedded image AND write each to
/// `dest_dir / img-<index>.<ext>`. `rel_to` is the directory the returned
/// `rel_path` is anchored at (typically the wiki root).
pub fn extract_and_save_images(
    path: &str,
    dest_dir: &Path,
    rel_to: &Path,
    options: &ExtractOptions,
) -> Result<Vec<SavedImage>, ExtractError> {
    use pdfium_render::prelude::*;

    let _guard = crate::core::files::lock_pdfium();
    let pdfium = crate::core::files::pdfium().map_err(|e| ExtractError::Pdfium(e))?;
    let doc = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|e| ExtractError::Pdfium(format!("Failed to open PDF '{path}': {e}")))?;

    let mut out: Vec<SavedImage> = Vec::new();
    let mut idx: u32 = 0;
    let mut total_objects: u32 = 0;
    let mut total_image_objects: u32 = 0;
    let mut filtered_too_small: u32 = 0;
    let mut filtered_decode_err: u32 = 0;
    let mut filtered_encode_err: u32 = 0;

    let page_count = doc.pages().len();
    eprintln!(
        "[extract_and_save_pdf_images] '{path}': {} page(s), filter=({}x{}) min, max={}",
        page_count, options.min_width, options.min_height, options.max_images
    );

    'pages: for (page_idx, page) in doc.pages().iter().enumerate() {
        for object in page.objects().iter() {
            total_objects += 1;
            let image = match object.as_image_object() {
                Some(img) => img,
                None => continue,
            };
            total_image_objects += 1;
            let dyn_img = match image.get_raw_image() {
                Ok(b) => b,
                Err(e) => {
                    filtered_decode_err += 1;
                    eprintln!(
                        "[extract_and_save_pdf_images] page {} image read failed: {e}",
                        page_idx + 1
                    );
                    continue;
                }
            };
            let width = dyn_img.width();
            let height = dyn_img.height();
            if width < options.min_width || height < options.min_height {
                filtered_too_small += 1;
                eprintln!(
                    "[extract_and_save_pdf_images] page {} image {}x{} < min ({}x{}) — skipped",
                    page_idx + 1,
                    width,
                    height,
                    options.min_width,
                    options.min_height
                );
                continue;
            }

            let mut png_bytes: Vec<u8> = Vec::new();
            if let Err(e) = dyn_img.write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            ) {
                filtered_encode_err += 1;
                eprintln!(
                    "[extract_and_save_pdf_images] page {} PNG encode failed: {e}",
                    page_idx + 1
                );
                continue;
            }

            idx += 1;
            let file_name = format!("img-{idx}.png");
            let (rel_path, abs_path) = save_one_image(&png_bytes, dest_dir, rel_to, &file_name)?;
            let sha256 = sha256_hex(&png_bytes);

            out.push(SavedImage {
                index: idx,
                mime_type: "image/png".to_string(),
                page: Some((page_idx + 1) as u32),
                width,
                height,
                rel_path,
                abs_path,
                sha256,
            });

            if out.len() >= options.max_images {
                eprintln!(
                    "[extract_and_save_pdf_images] reached max_images={} cap; skipped rest",
                    options.max_images
                );
                break 'pages;
            }
        }
    }

    eprintln!(
        "[extract_and_save_pdf_images] '{path}' DONE — saved={}, total_objects={}, image_objects={}, too_small={}, decode_err={}, encode_err={}",
        out.len(), total_objects, total_image_objects, filtered_too_small, filtered_decode_err, filtered_encode_err,
    );

    Ok(out)
}
