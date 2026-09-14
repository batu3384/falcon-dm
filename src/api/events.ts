import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { DownloadSchema, ProgressPayloadSchema } from '../lib/schema';
import type { DownloadModel, ProgressPayload } from '../types';

export function parseDownloadEvent(payload: unknown): DownloadModel | null {
  const parsed = DownloadSchema.safeParse(payload);
  return parsed.success ? parsed.data : null;
}

export function parseProgressEvent(payload: unknown): ProgressPayload | null {
  const parsed = ProgressPayloadSchema.safeParse(payload);
  return parsed.success ? parsed.data : null;
}

export function parsePairExtensionId(payload: unknown): string | null {
  if (!payload || typeof payload !== 'object') return null;
  const id = (payload as { extension_id?: unknown }).extension_id;
  if (typeof id !== 'string') return null;
  const trimmed = id.trim();
  return trimmed || null;
}

// ponytail: typed event listeners with zod validation. Each listen wrapper
// parses the payload before handing it to the callback, so a malformed backend
// event can't corrupt UI state. Returns the raw unlisten handle for cleanup.

export async function onDownloadAdded(cb: (d: DownloadModel) => void): Promise<UnlistenFn> {
  return listen<unknown>('download-added', (event) => {
    const parsed = parseDownloadEvent(event.payload);
    if (!parsed) return;
    cb(parsed);
  });
}

export async function onDownloadProgress(cb: (p: ProgressPayload) => void): Promise<UnlistenFn> {
  return listen<unknown>('download-progress', (event) => {
    const parsed = parseProgressEvent(event.payload);
    if (!parsed) return;
    cb(parsed);
  });
}

export async function onPairRequest(cb: (extensionId: string) => void): Promise<UnlistenFn> {
  return listen<unknown>('pair-request', (event) => {
    const id = parsePairExtensionId(event.payload);
    if (!id) return;
    cb(id);
  });
}
