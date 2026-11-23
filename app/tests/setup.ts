// Test setup: jsdom lacks a few browser APIs the window uses.
if (!window.matchMedia) {
  window.matchMedia = ((query: string) => ({
    matches: false, media: query, onchange: null,
    addListener() {}, removeListener() {}, addEventListener() {}, removeEventListener() {}, dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
}
if (!Element.prototype.scrollIntoView) Element.prototype.scrollIntoView = function () {};
if (!Element.prototype.scrollTo) Element.prototype.scrollTo = function () {} as typeof Element.prototype.scrollTo;
if (!('ResizeObserver' in window)) {
  (window as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}
// Svelte transitions use the Web Animations API, which jsdom lacks: finish at once.
if (!Element.prototype.animate) {
  Element.prototype.animate = function () {
    const anim = {
      onfinish: null as null | (() => void),
      oncancel: null,
      currentTime: 0,
      playState: 'finished',
      cancel() {},
      finish() {},
      pause() {},
      play() {},
      reverse() {},
      finished: Promise.resolve(),
      addEventListener() {},
      removeEventListener() {},
    };
    setTimeout(() => anim.onfinish?.(), 0);
    return anim as unknown as Animation;
  } as unknown as typeof Element.prototype.animate;
}
