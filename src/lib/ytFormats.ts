/** Quality presets offered for YouTube links. `null` = engine default (≤1080p MP4). */
export const YT_QUALITIES = [
  { id: 'best', format: null },
  {
    id: '2160',
    format: 'bestvideo[height<=2160]+bestaudio/best[height<=2160]',
  },
  {
    id: '1440',
    format: 'bestvideo[height<=1440]+bestaudio/best[height<=1440]',
  },
  {
    id: '720',
    format:
      'bestvideo[height<=720][ext=mp4]+bestaudio[ext=m4a]/bestvideo[height<=720]+bestaudio/best[height<=720]',
  },
  {
    id: '480',
    format:
      'bestvideo[height<=480][ext=mp4]+bestaudio[ext=m4a]/bestvideo[height<=480]+bestaudio/best[height<=480]',
  },
  { id: 'audio', format: 'bestaudio[ext=m4a]/bestaudio' },
] as const;

export type YtQualityId = (typeof YT_QUALITIES)[number]['id'];

export function ytFormatFor(id: YtQualityId): string | undefined {
  return YT_QUALITIES.find((q) => q.id === id)?.format ?? undefined;
}
