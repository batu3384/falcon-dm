export function formatBytes(bytes: number, decimals = 1): string {
  if (!bytes || bytes <= 0) return '0 B';
  const k = 1024;
  const dm = decimals < 0 ? 0 : decimals;
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(k)), sizes.length - 1);
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(dm))} ${sizes[i]}`;
}

export function calculateETA(remainingBytes: number, speed: number): string {
  if (speed <= 0 || remainingBytes <= 0) return '';
  const seconds = Math.ceil(remainingBytes / speed);
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) {
    const m = Math.floor(seconds / 60);
    const s = seconds % 60;
    return `${m}m ${s}s`;
  }
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  return `${h}h ${m}m`;
}

export function progressPercent(dl: { total_size: number; downloaded_size: number }): number {
  if (dl.total_size <= 0) return 0;
  return Math.min(100, Math.round((dl.downloaded_size / dl.total_size) * 100));
}

export function fileExtension(filename: string): string {
  return filename.includes('.') ? filename.split('.').pop()?.toUpperCase() || '' : '';
}

export function fileFullPath(dl: { save_path: string; filename: string }): string {
  const base = dl.save_path.replace(/\/$/, '');
  return `${base}/${dl.filename}`;
}
