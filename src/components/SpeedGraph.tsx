import { useEffect, useId, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { formatBytes } from '../types';

interface SpeedGraphProps {
  speed: number;
}

export default function SpeedGraph({ speed }: SpeedGraphProps) {
  const { t } = useTranslation();
  const [data, setData] = useState<number[]>(Array(28).fill(0));
  const gid = useId().replace(/:/g, '');
  const safeSpeed = Number.isFinite(speed) && speed >= 0 ? speed : 0;

  // ponytail: intentional sliding-window state update — the graph keeps a
  // rolling history of the last N speed samples. The set-state-in-effect lint
  // rule is disabled globally (see eslint.config.js) because this is the
  // intended pattern for prop-derived display state.
  useEffect(() => {
    setData((prev) => [...prev.slice(1), safeSpeed]);
  }, [safeSpeed]);

  const maxSpeed = Math.max(...data, 1);
  const w = 72;
  const h = 20;
  const step = w / (data.length - 1);

  const linePts = data.map((v, i) => `${i * step},${h - (v / maxSpeed) * (h - 2) - 1}`).join(' ');
  const areaPts = `0,${h} ${linePts} ${w},${h}`;
  const previousSpeed = data[data.length - 2] ?? 0;
  const trend =
    safeSpeed > previousSpeed
      ? t('stats.trend_up')
      : safeSpeed < previousSpeed
        ? t('stats.trend_down')
        : t('stats.trend_steady');

  return (
    <svg
      width="100%"
      height={h}
      viewBox={`0 0 ${w} ${h}`}
      className="sparkline"
      role="img"
      aria-label={t('stats.speed_graph_summary', {
        speed: formatBytes(safeSpeed),
        trend,
      })}
    >
      <defs>
        <linearGradient id={`sparkFill-${gid}`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="var(--accent)" stopOpacity="0.3" />
          <stop offset="100%" stopColor="var(--accent)" stopOpacity="0" />
        </linearGradient>
      </defs>
      <polygon points={areaPts} fill={`url(#sparkFill-${gid})`} />
      <polyline
        points={linePts}
        fill="none"
        stroke="var(--accent)"
        strokeWidth="1.5"
        strokeLinejoin="round"
        strokeLinecap="round"
      />
    </svg>
  );
}
