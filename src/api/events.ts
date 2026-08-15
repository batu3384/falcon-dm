import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { DownloadSchema, ProgressPayloadSchema } from '../lib/schema';
import type { DownloadModel, ProgressPayload } from '../types';

// ponytail: typed event listeners with zod validation. Each listen wrapper
// parses the payload before handing it to the callback, so a malformed backend
// event can't corrupt UI state. Returns the raw unlisten handle for cleanup.

export async function onDownloadAdded(cb: (d: DownloadModel) => void): Promise<UnlistenFn> {
  return listen<unknown>('download-added', (event) => {
    cb(DownloadSchema.parse(event.payload));
  });
}

export async function onDownloadProgress(cb: (p: ProgressPayload) => void): Promise<UnlistenFn> {
  return listen<unknown>('download-progress', (event) => {
    cb(ProgressPayloadSchema.parse(event.payload));
  });
}

export async function onPairRequest(cb: (extensionId: string) => void): Promise<UnlistenFn> {
  return listen<{ extension_id: string }>('pair-request', (event) => {
    cb(event.payload.extension_id);
  });
}
