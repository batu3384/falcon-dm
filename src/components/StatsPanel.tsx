import { useState, useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import {
  X,
  Activity,
  Clock,
  Pause,
  CheckCircle2,
  AlertCircle,
  Gauge,
  Database,
} from 'lucide-react';
import { useModalA11y } from '../hooks/useModalA11y';
import SpeedGraph from './SpeedGraph';
import * as api from '../api/commands';
import type { DownloadStats } from '../api/commands';
import { formatBytes } from '../types';

interface StatsPanelProps {
  onClose: () => void;
}

// ponytail: the Stats panel gives an at-a-glance operational picture — KPI
// cards (Active/Queued/Completed/Failed), totals (data downloaded, current
// speed), and a live speed sparkline. Auto-refreshes every 2s while open.
export function StatsPanel({ onClose }: StatsPanelProps) {
  const { t } = useTranslation();
  const panelRef = useRef<HTMLDivElement>(null);
  useModalA11y(panelRef, onClose);
  const [stats, setStats] = useState<DownloadStats | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [retryNonce, setRetryNonce] = useState(0);

  useEffect(() => {
    let cancelled = false;
    const tick = async () => {
      try {
        const s = await api.getStats();
        if (!cancelled) {
          setStats(s);
          setError(null);
        }
      } catch (e) {
        if (!cancelled) setError(api.extractTauriError(e));
      }
    };
    tick();
    const id = setInterval(tick, 2000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, [retryNonce]);

  const cards = [
    {
      key: 'active',
      label: t('stats.active'),
      value: stats?.active ?? 0,
      Icon: Activity,
      dot: 'active',
    },
    {
      key: 'queued',
      label: t('stats.queued'),
      value: stats?.queued ?? 0,
      Icon: Clock,
      dot: 'queued',
    },
    {
      key: 'paused',
      label: t('stats.paused'),
      value: stats?.paused ?? 0,
      Icon: Pause,
      dot: 'queued',
    },
    {
      key: 'completed',
      label: t('stats.completed'),
      value: stats?.completed ?? 0,
      Icon: CheckCircle2,
      dot: 'completed',
    },
    {
      key: 'failed',
      label: t('stats.failed'),
      value: stats?.failed ?? 0,
      Icon: AlertCircle,
      dot: 'failed',
    },
  ] as const;

  return (
    <div className="modal-overlay" onClick={onClose} role="presentation">
      <div
        ref={panelRef}
        className="modal-panel modal-lg stats-panel"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-labelledby="stats-title"
      >
        <div className="modal-head">
          <h2 id="stats-title" className="modal-title">
            {t('stats.title')}
          </h2>
          <button
            type="button"
            onClick={onClose}
            className="icon-btn"
            data-modal-cancel
            aria-label={t('stats.close')}
          >
            <X size={18} />
          </button>
        </div>

        <div className="modal-body">
          {error && stats && (
            <div className="diagnostic-error" role="alert">
              <span title={error}>{t('stats.load_error')}</span>
              <button
                type="button"
                className="btn-secondary"
                onClick={() => setRetryNonce((value) => value + 1)}
              >
                {t('stats.retry')}
              </button>
            </div>
          )}
          {!stats && error ? (
            <div className="empty-state" role="alert">
              <AlertCircle size={24} />
              <span className="empty-title">{t('stats.load_error')}</span>
              <button
                type="button"
                className="btn-primary"
                onClick={() => setRetryNonce((value) => value + 1)}
              >
                {t('stats.retry')}
              </button>
            </div>
          ) : (
            <>
              <div className="stats-grid">
                {cards.map((c) => (
                  <div className="kpi-card" key={c.key}>
                    <div className="kpi-card-head">
                      <c.Icon size={13} />
                      <span className={`kpi-dot ${c.dot}`} />
                      {c.label}
                    </div>
                    <div className="kpi-value">{c.value}</div>
                  </div>
                ))}
              </div>

              <div className="stats-totals">
                <div className="stats-total-item">
                  <span className="stats-total-label">
                    <Database
                      size={11}
                      style={{ display: 'inline', verticalAlign: '-1px', marginRight: 4 }}
                    />
                    {t('stats.total_downloaded')}
                  </span>
                  <span className="stats-total-value">
                    {formatBytes(stats?.total_downloaded_bytes ?? 0)}
                  </span>
                </div>
                <div className="stats-total-item">
                  <span className="stats-total-label">
                    <Gauge
                      size={11}
                      style={{ display: 'inline', verticalAlign: '-1px', marginRight: 4 }}
                    />
                    {t('stats.current_speed')}
                  </span>
                  <span className="stats-total-value">
                    {formatBytes(stats?.current_speed ?? 0)}/s
                  </span>
                </div>
              </div>

              <div className="stats-chart">
                <div className="stats-total-label" style={{ marginBottom: 8 }}>
                  {t('stats.speed_over_time')}
                </div>
                <div style={{ width: '100%', display: 'flex', justifyContent: 'center' }}>
                  <SpeedGraph speed={stats?.current_speed ?? 0} />
                </div>
              </div>
            </>
          )}
        </div>

        <div className="modal-foot">
          <button type="button" className="btn-secondary" data-modal-cancel onClick={onClose}>
            {t('stats.close')}
          </button>
        </div>
      </div>
    </div>
  );
}
