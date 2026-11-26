import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';

import enTranslation from './locales/en.json';
import trTranslation from './locales/tr.json';

const LANG_KEY = 'falcon_lang';

function systemLanguage(): 'en' | 'tr' {
  const lang = (typeof navigator !== 'undefined' && navigator.language) || 'en';
  return lang.toLowerCase().startsWith('tr') ? 'tr' : 'en';
}

/** 'system' | 'en' | 'tr' as chosen in Settings. */
export function languagePreference(): string {
  const saved = localStorage.getItem(LANG_KEY);
  return saved === 'en' || saved === 'tr' ? saved : 'system';
}

function resolve(preference: string): 'en' | 'tr' {
  return preference === 'en' || preference === 'tr' ? preference : systemLanguage();
}

export function applyLanguagePreference(preference: string) {
  if (preference === 'en' || preference === 'tr') localStorage.setItem(LANG_KEY, preference);
  else localStorage.removeItem(LANG_KEY);
  const lang = resolve(preference);
  document.documentElement.lang = lang;
  void i18n.changeLanguage(lang);
}

const initial = resolve(languagePreference());
document.documentElement.lang = initial;

void i18n.use(initReactI18next).init({
  resources: {
    en: { translation: enTranslation },
    tr: { translation: trTranslation },
  },
  lng: initial,
  fallbackLng: 'en',
  interpolation: { escapeValue: false },
});

export default i18n;
