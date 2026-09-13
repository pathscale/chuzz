#[test]
fn event_listener_can_reuse_a_memoized_getter() {
    let mut document = blitz_script::ScriptDocument::from_html(
        "<html><body><input id='field'></body></html>",
        blitz_dom::DocumentConfig::default(),
    );
    document.eval(
        "globalThis.memoCalls = 0;\
             class Fixture {\
               get handle() {\
                 globalThis.memoCalls += 1;\
                 const value = { input: '#field' };\
                 Object.defineProperty(this, 'handle', { value });\
                 return this.handle;\
               }\
               attach(element) {\
                 element.addEventListener('input', () => this.handle.input);\
               }\
             }\
             const fixture = new Fixture();\
             const field = document.getElementById('field');\
             fixture.attach(field);\
             globalThis.fixture = fixture;\
             globalThis.field = field;",
    );
    let first = document
        .eval_json(
            "field.dispatchEvent(new Event('input'));\
             [memoCalls, Object.hasOwn(fixture, 'handle'), fixture.handle.input]",
        )
        .expect("the first listener call should define the memoized value");
    assert_eq!(first, serde_json::json!([1, true, "#field"]));

    let second = document
        .eval_json(
            "field.dispatchEvent(new Event('input'));\
             [memoCalls, Object.hasOwn(fixture, 'handle'), fixture.handle.input]",
        )
        .expect("the second listener call should read the memoized value");
    assert_eq!(second, serde_json::json!([1, true, "#field"]));
}
