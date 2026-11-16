use std::sync::Arc;

#[test]
fn event_listener_can_reuse_a_memoized_getter() {
    let base = url::Url::parse("https://fixture.invalid/").expect("fixture URL");
    let lifetime = blitz_dom::release::Teardown::new();
    let signal = blitz_traits::net::AbortSignal::default();
    let net = Arc::new(chuzz::document_loader::NetProvider::new(None));
    let config = blitz_dom::DocumentConfig {
        base_url: Some(base.to_string()),
        html_parser_provider: Some(Arc::new(blitz_html::HtmlProvider)),
        ..Default::default()
    };
    let host_config = chuzz::script_host::HostInstallContext {
        net,
        base: Some(base),
        deadline: std::time::Duration::from_secs(15),
        wake: chuzz::net_bridge::PageWake::default(),
        lifetime,
        signal,
        session_backing: None,
    };
    let mut document = chuzz::page_document::PageDocument::from_html(
        Arc::<str>::from("<html><body><input id='field'></body></html>"),
        config,
        host_config,
    )
    .expect("construct with the registered page runtime");
    document
        .eval(
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
        )
        .expect("install the event listener");
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
