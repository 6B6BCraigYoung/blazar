use std::io::Cursor;

use image::{ImageFormat, Rgba, RgbaImage};

use super::*;

fn png(width: u32, height: u32, alpha: u8) -> Vec<u8> {
    let img = RgbaImage::from_pixel(width, height, Rgba([200, 30, 30, alpha]));
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .unwrap();
    out
}

#[test]
fn small_images_go_through_untouched() {
    let bytes = png(64, 32, 255);
    let p = prepare(&bytes, MAX_SIDE, PASS_BYTES).unwrap();
    assert_eq!(p.mime, "image/png");
    assert_eq!((p.width, p.height), (64, 32));
    assert_eq!(p.shown, (64, 32));
    assert_eq!(p.data, bytes);
}

#[test]
fn large_images_shrink_to_the_longest_side_as_jpeg() {
    let bytes = png(2000, 1000, 255);
    let p = prepare(&bytes, MAX_SIDE, PASS_BYTES).unwrap();
    assert_eq!(p.mime, "image/jpeg");
    assert_eq!((p.width, p.height), (2000, 1000));
    assert_eq!(p.shown, (1568, 784));
    assert_eq!(dimensions(&p.data).unwrap(), (1568, 784));
}

#[test]
fn heavy_files_are_reencoded_even_when_small_enough_in_pixels() {
    let bytes = png(300, 300, 255);
    let p = prepare(&bytes, MAX_SIDE, 10).unwrap();
    assert_eq!(p.mime, "image/jpeg");
    assert_eq!(p.shown, (300, 300));
}

#[test]
fn see_through_pixels_land_on_white() {
    let clear = png(10, 10, 0);
    let p = prepare(&clear, MAX_SIDE, 10).unwrap();
    let img = image::load_from_memory(&p.data).unwrap().to_rgb8();
    let px = img.get_pixel(5, 5);
    assert!(px.0.iter().all(|c| *c > 240), "{px:?}");
}

#[test]
fn image_files_are_known_by_their_extension() {
    for p in ["a.png", "b.JPG", "c.jpeg", "d.webp", "e.gif", "dir/f.PNG"] {
        assert!(is_image(p), "{p}");
    }
    for p in ["a.txt", "b.svg", "c.pdf", "png"] {
        assert!(!is_image(p), "{p}");
    }
    assert!(prepare(b"not an image", MAX_SIDE, PASS_BYTES).is_err());
}
