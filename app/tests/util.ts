import { render } from '@testing-library/svelte';
import App from '../src/App.svelte';
import { setBackend } from '../src/lib/backend';
import { createMockBackend } from '../src/lib/mock';
import { app, resetState, start } from '../src/lib/state/app.svelte';
import { setLanguage } from '../src/lib/i18n/index.svelte';

export type Mock = ReturnType<typeof createMockBackend>;

/** Renders the whole window against a fresh mock backend at `hash`. */
export async function boot(query = '', hash = '#/', wait = true) {
  resetState();
  await setLanguage('en');
  const backend = createMockBackend(new URLSearchParams(query), { latency: 0 });
  setBackend(backend);
  history.replaceState(null, '', (query ? '/?' + query : '/') + hash);
  const r = render(App);
  if (wait) await start();
  else void start();
  return { ...r, backend, service: backend.service, app };
}

export function calls(m: Mock, method: string) {
  return m.service.calls.filter((c) => c.method === method).map((c) => c.params);
}
