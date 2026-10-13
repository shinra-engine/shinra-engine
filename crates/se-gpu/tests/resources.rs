use se_gpu::resources::{decode, decode_png, parse_obj};

#[test]
fn flat_normals_when_no_vn() {
    let m = parse_obj("v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nf 1 2 3\nf 2 4 3\n").unwrap();
    assert_eq!(m.vertices.len(), 6);
    assert_eq!(&m.vertices[0][3..], &[0.0, 0.0, 1.0]);
}

#[test]
fn vn_is_used_and_quad_fans() {
    let m = parse_obj(
        "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvn 0 0 -1\nf 1//1 2//1 3//1 4//1\n",
    )
    .unwrap();
    assert_eq!(m.vertices.len(), 6);
    assert_eq!(&m.vertices[5][3..], &[0.0, 0.0, -1.0]);
}

#[test]
fn negative_indices() {
    let m = parse_obj("v 0 0 0\nv 1 0 0\nv 0 1 0\nf -3 -2 -1\n").unwrap();
    assert_eq!(m.vertices.len(), 3);
}

#[test]
fn malformed_face_names_its_line() {
    let e = parse_obj("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 9\n").unwrap_err();
    assert!(e.to_string().contains("line 4"), "{e}");
}

#[test]
fn png_round_trips() {
    use image::ImageEncoder;
    let px: Vec<u8> = (0..16).collect();
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&px, 2, 2, image::ExtendedColorType::Rgba8)
        .unwrap();
    let d = decode_png(&bytes).unwrap();
    assert_eq!((d.width, d.height), (2, 2));
    assert_eq!(d.rgba, px);
}

#[test]
fn other_kinds_are_blobs() {
    assert!(decode("txt", b"hi").unwrap().is_none());
}
