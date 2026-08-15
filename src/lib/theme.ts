export function applyTheme(theme: string) {
  // ponytail: a manual theme choice sets data-theme-manual="true" so the OS
  // theme-change listener no longer clobbers the user's explicit preference.
  // Previously applyTheme never set that flag, so toggling dark/light in
  // Settings was silently overridden the moment the OS theme changed.
  if (theme === 'dark' || theme === 'light') {
    document.documentElement.setAttribute('data-theme', theme);
    document.documentElement.setAttribute('data-theme-manual', 'true');
  } else {
    document.documentElement.removeAttribute('data-theme-manual');
    const dark = window.matchMedia('(prefers-color-scheme: dark)').matches;
    document.documentElement.setAttribute('data-theme', dark ? 'dark' : 'light');
  }
}

// ponytail: matchMedia listener kept simple — OS theme change while app is open should update instantly.
let _themeMq: MediaQueryList | null = null;
export function watchSystemTheme() {
  if (_themeMq) return;
  _themeMq = window.matchMedia('(prefers-color-scheme: dark)');
  const handler = () => {
    if (document.documentElement.getAttribute('data-theme-manual') !== 'true') {
      document.documentElement.setAttribute('data-theme', _themeMq!.matches ? 'dark' : 'light');
    }
  };
  _themeMq.addEventListener('change', handler);
}
