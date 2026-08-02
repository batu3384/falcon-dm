import { useState, useEffect, useRef, useCallback, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { X, RefreshCw, Trash2, Search, AlertCircle, AlertTriangle, Info, Bug } from 'lucide-react';
import { useModalA11y } from '../hooks/useModalA11y';
import { useToastStore } from '../store/toast';
import * as api from '../api/commands';
import type { LogEntry } from '../api/commands';

interface LogPanelProps {
  onClose: () => void;
}

type LevelFilter = 'ALL' | 'ERROR' | 'WARN' | 'INFO' | 'DEBUG';

// ponytail: the Log panel surfaces the backend's in-memory ring buffer so users
// (and support) can see what the engine is doing — aria2 RPC state, queue tick
// warnings, transient retries, pairing events — without launching a terminal.
const LEVELS: LevelFilter[] = ['ALL', 'ERROR', 'WARN', 'INFO', 'DEBUG'];

function levelMeta(level: string) {
  switch (level) {
    case 'ERROR':
      return { Icon: AlertCircle, cls: 'error', color: 'var(--danger)' };
    case 'WARN':
      return { Icon: AlertTriangle, cls: 'warn', color: 'var(--warning, #d97706)' };
    case 'DEBUG':
    case 'TRACE':
      return { Icon: Bug, cls: 'debug', color: 'var(--text-3)' };
    default:
      return { Icon: Info, cls: 'info', color: 'var(--accent)' };
  }
}

function formatTs(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

export function LogPanel({ onClose }: LogPanelProps) {
  const { t } = useTranslation();
  const showToast = useToastStore((s) => s.showToast);
  const panelRef = useRef<HTMLDivElement>(null);
  useModalA11y(panelRef, onClose);

  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [level, setLevel] = useState<LevelFilter>('ALL');
  const [query, setQuery] = useState('');
  const [autoRefresh, setAutoRefresh] = useState(true);
  const scrollRef = useRef<HTMLDivElement>(null);

  const refresh = useCallback(async () => {
    try {
      const data = await api.getLogs(level === 'ALL' ? undefined : level);
      setLogs(data);
    } catch (e) {
      showToast('error', api.extractTauriError(e));
    }
  }, [level, showToast]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  // Auto-refresh every 2s while open.
  useEffect(() => {
    if (!autoRefresh) return;
    const id = setInterval(refresh, 2000);
    return () => clearInterval(id);
  }, [autoRefresh, refresh]);

  // Auto-scroll to bottom on new logs (only if user is already near the bottom).
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
    if (nearBottom) {
      el.scrollTop = el.scrollHeight;
    }
  }, [logs]);

  const handleClear = useCallback(async () => {
    try {
      await api.clearLogs();
      setLogs([]);
      showToast('success', t('logs.cleared'));
    } catch (e) {
      showToast('error', api.extractTauriError(e));
    }
  }, [showToast, t]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return logs;
    return logs.filter(
      (l) => l.message.toLowerCase().includes(q) || l.target.toLowerCase().includes(q),
    );
  }, [logs, query]);

  return (
    <div className="modal-overlay" onClick={onClose} role="presentation">
      <div
        ref={panelRef}
        className="modal-panel modal-lg log-panel"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-labelledby="logs-title"
      >
        <div className="modal-head">
          <h2 id="logs-title" className="modal-title">
            {t('logs.title')}
          </h2>
          <button type="button" onClick={onClose} className="icon-btn" aria-label={t('logs.close')}>
            <X size={18} />
          </button>
        </div>

        <div className="modal-body log-toolbar">
          <div className="log-levels" role="tablist" aria-label={t('logs.level_filter')}>
            {LEVELS.map((lv) => (
              <button
                key={lv}
                type="button"
                role="tab"
                aria-selected={level === lv}
                className={`log-level-tab ${level === lv ? 'active' : ''}`}
                onClick={() => setLevel(lv)}
              >
                {lv}
              </button>
            ))}
          </div>
          <div className="search-wrap log-search">
            <Search size={14} />
            <input
              className="search-input"
              placeholder={t('logs.search')}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              aria-label={t('logs.search')}
            />
          </div>
          <button
            type="button"
            className={`icon-btn ${autoRefresh ? 'active' : ''}`}
            onClick={() => setAutoRefresh((v) => !v)}
            title={t('logs.auto_refresh')}
            aria-label={t('logs.auto_refresh')}
            aria-pressed={autoRefresh}
          >
            <RefreshCw size={15} className={autoRefresh ? 'spin-slow' : ''} />
          </button>
          <button
            type="button"
            className="icon-btn danger"
            onClick={handleClear}
            title={t('logs.clear')}
            aria-label={t('logs.clear')}
          >
            <Trash2 size={15} />
          </button>
        </div>

        <div
          className="log-list"
          ref={scrollRef}
          role="log"
          aria-live="polite"
          aria-label={t('logs.stream')}
        >
          {filtered.length === 0 ? (
            <div className="empty-state">
              <span className="empty-title">{t('logs.empty')}</span>
            </div>
          ) : (
            filtered.map((entry, i) => {
              const { Icon, cls, color } = levelMeta(entry.level);
              return (
                <div key={i} className={`log-row log-${cls}`}>
                  <span className="log-ts mono">{formatTs(entry.ts)}</span>
                  <span className="log-level-badge" style={{ color }}>
                    <Icon size={11} />
                    {entry.level}
                  </span>
                  <span className="log-target mono">{entry.target}</span>
                  <span className="log-message">{entry.message}</span>
                </div>
              );
            })
          )}
        </div>

        <div className="modal-foot log-foot">
          <span className="log-count mono">
            {filtered.length} {t('logs.entries')}
          </span>
          <button type="button" className="btn-secondary" onClick={onClose}>
            {t('logs.close')}
          </button>
        </div>
      </div>
    </div>
  );
}
