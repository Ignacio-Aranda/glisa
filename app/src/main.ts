import '@fontsource-variable/inter';
import './lib/brand/tokens.css';
import './app.css';
import { mount } from 'svelte';
import App from './App.svelte';
import { start } from './lib/state/app.svelte';

// Screenshot runs (scripts/screenshots.mjs): keep the load event pending while the window draws.
const shot = new URLSearchParams(location.search).get('shot');
if (shot) {
  const img = document.createElement('img');
  img.src = `/__shot-delay?ms=${Number(shot) || 1500}`;
  img.alt = '';
  img.style.display = 'none';
  document.body.appendChild(img);
}

// It's an app, not a web page: no browser context menu (Open Link, Download…) except in text
// fields, and no opening links in new windows with modifier or middle clicks.
function editable(el: EventTarget | null): boolean {
  const e = el as HTMLElement | null;
  return !!e?.closest?.('input, textarea, [contenteditable="true"], .selectable');
}
document.addEventListener('contextmenu', (e) => {
  if (!editable(e.target)) e.preventDefault();
});
document.addEventListener('click', (e) => {
  const a = (e.target as HTMLElement | null)?.closest?.('a[href]');
  if (a && (e.ctrlKey || e.metaKey || e.shiftKey)) e.preventDefault();
}, true);
document.addEventListener('auxclick', (e) => {
  if ((e.target as HTMLElement | null)?.closest?.('a[href]')) e.preventDefault();
});
document.addEventListener('dragstart', (e) => {
  if ((e.target as HTMLElement | null)?.closest?.('a[href], img')) e.preventDefault();
});

const target = document.getElementById('app')!;
mount(App, { target });
void start();
