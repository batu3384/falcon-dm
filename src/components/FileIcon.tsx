import { AppWindow, File, FileArchive, FileText, Film, Music } from 'lucide-react';
import type { DownloadCategory } from '../types';

const KIND: Record<DownloadCategory, { Icon: typeof File; tint: string }> = {
  Video: { Icon: Film, tint: 'var(--tint-video)' },
  Music: { Icon: Music, tint: 'var(--tint-music)' },
  Document: { Icon: FileText, tint: 'var(--tint-document)' },
  Archive: { Icon: FileArchive, tint: 'var(--tint-archive)' },
  Program: { Icon: AppWindow, tint: 'var(--tint-program)' },
  Other: { Icon: File, tint: 'var(--tint-other)' },
};

export function FileIcon({
  category,
  size = 18,
  className,
}: {
  category: DownloadCategory;
  size?: number;
  className: string;
}) {
  const { Icon, tint } = KIND[category] ?? KIND.Other;
  return (
    <span
      className={className}
      style={{ '--tint': tint } as React.CSSProperties}
      aria-hidden="true"
    >
      <Icon size={size} strokeWidth={1.7} />
    </span>
  );
}
