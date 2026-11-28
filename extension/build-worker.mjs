#!/usr/bin/env node
/**
 * Concatenate the ordered background scripts for Chromium's single-file service worker.
 * Firefox loads the same source files through manifest background.scripts.
 */
import { readFileSync, writeFileSync } from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const dir = path.dirname(fileURLToPath(import.meta.url));
export const BACKGROUND_FILES = ['media-utils.js', 'shared.js', 'pairing.js', 'api.js', 'background.js'];

export function buildBackgroundWorker() {
  const body = BACKGROUND_FILES.map((file) =>
    readFileSync(path.join(dir, file), 'utf8').trimEnd(),
  ).join('\n\n');
  return `/* Bundled Chromium service worker; edit the ordered source files instead. */\n${body}\n`;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  writeFileSync(path.join(dir, 'background-worker.js'), buildBackgroundWorker());
  console.log('background-worker.js written');
}
