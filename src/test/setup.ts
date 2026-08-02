import '@testing-library/jest-dom/vitest';
import { afterEach } from 'vitest';
import { cleanup } from '@testing-library/react';

// ponytail: auto-cleanup DOM between tests so renders don't leak across cases.
afterEach(() => {
  cleanup();
});

// jsdom doesn't implement matchMedia — components that read the OS theme need it.
if (!window.matchMedia) {
  window.matchMedia = (query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  });
}
