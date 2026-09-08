// Polyfills for the jsdom environment.

// reka-ui primitives observe their own size on mount; jsdom implements neither of these.
globalThis.ResizeObserver = class ResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
};

if (!Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = function () {};
}
