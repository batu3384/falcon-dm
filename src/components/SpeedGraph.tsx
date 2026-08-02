import { useEffect, useId, useState } from 'react';

interface SpeedGraphProps {
  speed: number;
}

export default function SpeedGraph({ speed }: SpeedGraphProps) {
  const [data, setData] = useState<number[]>(Array(28).fill(0));
  const gid = useId().replace(/:/g, '');

  // ponytail: intentional sliding-window state update — the graph keeps a
  // rolling history of the last N speed samples. The set-state-in-effect lint
  // rule is disabled globally (see eslint.config.js) because this is the
  // intended pattern for prop-derived display state.
  useEffect(() => {
    setData((prev) => [...prev.slice(1), speed]);
  }, [speed]);

  const maxSpeed = Math.max(...data, 1);
  const w = 72;
  const h = 20;
  const step = w / (data.length - 1);

  const linePts = data.map((v, i) => `${i * step},${h - (v / maxSpeed) * (h - 2) - 1}`).join(' ');
  const areaPts = `0,${h} ${linePts} ${w},${h}`;

  return (
    <svg width={w} height={h} className="sparkline" aria-hidden="true">
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
