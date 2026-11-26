import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { DownloadSchema, PairRequestSchema, ProgressPayloadSchema } from '../lib/schema';
import type { DownloadModel, PairRequest, ProgressPayload } from '../types';

export function parseDownloadEvent(payload: unknown): DownloadModel | null {
  const parsed = DownloadSchema.safeParse(payload);
  return parsed.success ? parsed.data : null;
}

export function parseProgressEvent(payload: unknown): ProgressPayload | null {
  const parsed = ProgressPayloadSchema.safeParse(payload);
  return parsed.success ? parsed.data : null;
}

export function parsePairRequest(payload: unknown): PairRequest | null {
  const parsed = PairRequestSchema.safeParse(payload);
  return parsed.success ? parsed.data : null;
}

export function parseRemovedId(payload: unknown): number | null {
  return typeof payload === 'number' && Number.isInteger(payload) ? payload : null;
}

// Each wrapper validates the payload before the callback sees it, so a
// malformed event cannot corrupt UI state.
function on<T>(
  name: string,
  parse: (payload: unknown) => T | null,
  cb: (value: T) => void,
): Promise<UnlistenFn> {
  return listen<unknown>(name, (event) => {
    const value = parse(event.payload);
    if (value !== null) cb(value);
  });
}

/** A new row was created (manual add, extension, upload). */
export const onDownloadAdded = (cb: (d: DownloadModel) => void) =>
  on('download-added', parseDownloadEvent, cb);

/** Authoritative row after any status or metadata change. */
export const onDownloadUpdated = (cb: (d: DownloadModel) => void) =>
  on('download-updated', parseDownloadEvent, cb);

export const onDownloadRemoved = (cb: (id: number) => void) =>
  on('download-removed', parseRemovedId, cb);

/** Byte counters of a running worker (about twice a second). */
export const onDownloadProgress = (cb: (p: ProgressPayload) => void) =>
  on('download-progress', parseProgressEvent, cb);

export const onPairRequest = (cb: (request: PairRequest) => void) =>
  on('pair-request', parsePairRequest, cb);
