//! The RandomNormalLike rewrite, run through real ONNX Runtime.

use cantor_backends::onnx::{Model, tensor_f32};
use cantor_render::rng::Rng;

/// x (float [2,3]) -> Identity -> RandomNormalLike -> y, opset 15.
fn tiny_model() -> Vec<u8> {
    // Serialized with the same minimal writer the rewrite uses; see
    // onnx_noise::tests::tiny_model. Kept here as bytes to test the public path.
    cantor_backends::onnx_noise::test_support::tiny_model()
}

#[test]
fn noise_comes_from_the_seed() {
    let dir = std::env::temp_dir().join(format!("cantor-noise-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tiny.onnx");
    std::fs::write(&path, tiny_model()).unwrap();
    let model = Model::load(&path).unwrap();

    let run = |seed| {
        let x = tensor_f32(&[2, 3], vec![0.0; 6]).unwrap();
        model
            .run(vec![("x", x)], seed, 64, &["y"])
            .unwrap()
            .remove(0)
    };
    let a = run(7);
    assert_eq!(a, run(7), "same seed, same noise");
    assert_ne!(a, run(8), "different seed, different noise");

    // The graph takes the first 6 samples of the seeded noise stream.
    let mut rng = Rng::new(7);
    assert_eq!(a, rng.normals_f32(6));
    std::fs::remove_dir_all(dir).unwrap();
}
