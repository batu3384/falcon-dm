import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';

import enTranslation from './locales/en.json';
import trTranslation from './locales/tr.json';

const saved = localStorage.getItem('falcon_lang');
const userLang = navigator.language || (navigator as any).userLanguage;
const defaultLang = saved || (userLang.startsWith('tr') ? 'tr' : 'en');

document.documentElement.lang = defaultLang;

i18n
  .use(initReactI18next)
  .init({
    resources: {
      en: { translation: enTranslation },
      tr: { translation: trTranslation }
    },
    lng: defaultLang,
    fallbackLng: 'en',
    interpolation: {
      escapeValue: false
    }
  });

export default i18n;
