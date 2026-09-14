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
  const [error, setError] = useState<string | null>(null);
  const [level, setLevel] = useState<LevelFilter>('ALL');
  const [query, setQuery] = useState('');
  const [autoRefresh, setAutoRefresh] = useState(true);
  const scrollRef = useRef<HTMLDivElement>(null);
  const errorToastSent = useRef(false);

  const refresh = useCallback(async () => {
    try {
      const data = await api.getLogs(level === 'ALL' ? undefined : level);
      setLogs(data);
      setError(null);
      errorToastSent.current = false;
    } catch (e) {
      const detail = api.extractTauriError(e);
      setError(detail);
      if (!errorToastSent.current) {
        errorToastSent.current = true;
        showToast('error', detail);
      }
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
          <button
            type="button"
            onClick={onClose}
            className="icon-btn"
            data-modal-cancel
            aria-label={t('logs.close')}
          >
            <X size={18} aria-hidden={true} />
          </button>
        </div>

        <div className="modal-body log-toolbar">
          <div
            className="log-levels"
            role="radiogroup"
            aria-label={t('logs.level_filter')}
            onKeyDown={(e) => {
              if (
                e.key !== 'ArrowRight' &&
                e.key !== 'ArrowLeft' &&
                e.key !== 'ArrowDown' &&
                e.key !== 'ArrowUp' &&
                e.key !== 'Home' &&
                e.key !== 'End'
              ) {
                return;
              }
              e.preventDefault();
              const i = LEVELS.indexOf(level);
              let next = i;
              if (e.key === 'ArrowRight' || e.key === 'ArrowDown') next = (i + 1) % LEVELS.length;
              else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp')
                next = (i - 1 + LEVELS.length) % LEVELS.length;
              else if (e.key === 'Home') next = 0;
              else next = LEVELS.length - 1;
              const lv = LEVELS[next];
              setLevel(lv);
              requestAnimationFrame(() => document.getElementById(`logs-tab-${lv}`)?.focus());
            }}
          >
            {LEVELS.map((lv) => (
              <button
                key={lv}
                type="button"
                id={`logs-tab-${lv}`}
                role="radio"
                aria-checked={level === lv}
                tabIndex={level === lv ? 0 : -1}
                className={`log-level-tab ${level === lv ? 'active' : ''}`}
                onClick={() => setLevel(lv)}
              >
                {lv}
              </button>
            ))}
          </div>
          <div className="search-wrap log-search">
            <Search size={14} aria-hidden={true} />
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
            <RefreshCw size={15} className={autoRefresh ? 'spin-slow' : ''} aria-hidden={true} />
          </button>
          <button
            type="button"
            className="icon-btn danger"
            onClick={handleClear}
            title={t('logs.clear')}
            aria-label={t('logs.clear')}
          >
            <Trash2 size={15} aria-hidden={true} />
          </button>
        </div>

        <div
          className="log-list"
          ref={scrollRef}
          role="log"
          aria-live="polite"
          aria-label={t('logs.stream')}
        >
          {error && logs.length > 0 && (
            <div className="diagnostic-error" role="alert">
              <span title={error}>{t('logs.load_error')}</span>
              <button type="button" className="btn-secondary" onClick={refresh}>
                {t('logs.retry')}
              </button>
            </div>
          )}
          {error && logs.length === 0 ? (
            <div className="empty-state" role="alert">
              <AlertCircle size={24} aria-hidden={true} />
              <span className="empty-title">{t('logs.load_error')}</span>
              <button type="button" className="btn-primary" onClick={refresh}>
                {t('logs.retry')}
              </button>
            </div>
          ) : filtered.length === 0 ? (
            <div className="empty-state">
              <span className="empty-title">{t('logs.empty')}</span>
            </div>
          ) : (
            filtered.map((entry) => {
              const { Icon, cls } = levelMeta(entry.level);
              const rowKey = `${entry.ts}-${entry.level}-${entry.target}-${entry.message.length}`;
              return (
                <div key={rowKey} className={`log-row log-${cls}`}>
                  <span className="log-ts mono">{formatTs(entry.ts)}</span>
                  <span className={`log-level-badge ${cls}`}>
                    <Icon size={11} aria-hidden={true} />
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
          <button type="button" className="btn-secondary" data-modal-cancel onClick={onClose}>
            {t('logs.close')}
          </button>
        </div>
      </div>
    </div>
  );
}
