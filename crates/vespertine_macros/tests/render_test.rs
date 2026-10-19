#[test]
fn test_derive_render() {
    use vespertine_macros::Render;

    #[derive(Render)]
    struct _Element;
}
