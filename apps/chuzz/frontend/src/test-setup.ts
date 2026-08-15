/**
 * What jsdom does not implement and `@pathscale/ui` reads at import time.
 *
 * The library's barrel pulls in every component, and a few of them ask the
 * environment a question on the way in. jsdom answers most of the DOM and none
 * of these, so importing the library at all throws before a single test runs.
 * Stubs rather than a mocking library: each one returns the quiet answer, which
 * is what a test that is not about media queries or resizing wants.
 */

if (!window.matchMedia) {
  window.matchMedia = (query: string) =>
    ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }) as MediaQueryList;
}

for (const name of ["ResizeObserver", "IntersectionObserver"] as const) {
  if (!(name in window)) {
    (window as unknown as Record<string, unknown>)[name] = class {
      observe() {}
      unobserve() {}
      disconnect() {}
      takeRecords() {
        return [];
      }
    };
  }
}
