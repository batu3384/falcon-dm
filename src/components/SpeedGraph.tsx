import { useEffect, useState } from "react";

interface SpeedGraphProps {
  speed: number; // current speed in bytes per second
}

export default function SpeedGraph({ speed }: SpeedGraphProps) {
  const [data, setData] = useState<number[]>(Array(20).fill(0));

  useEffect(() => {
    setData((prevData) => {
      const newData = [...prevData, speed];
      if (newData.length > 20) {
        newData.shift();
      }
      return newData;
    });
  }, [speed]);

  const maxSpeed = Math.max(...data, 1); // Avoid division by zero
  
  return (
    <svg width="60" height="20" className="speed-graph" style={{ margin: '0 8px' }}>
      <polyline
        fill="none"
        stroke="#4a90e2"
        strokeWidth="1.5"
        points={data
          .map((value, index) => {
            const x = (index / (20 - 1)) * 60;
            const y = 20 - (value / maxSpeed) * 20;
            return `${x},${y}`;
          })
          .join(" ")}
      />
    </svg>
  );
}
