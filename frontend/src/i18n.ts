import { createI18n } from 'vue-i18n'
import en from './locales/en'
import zh from './locales/zh'

// Function to get the initial locale
function getInitialLocale(): 'zh' | 'en' {
  const savedLocale = localStorage.getItem('vust_locale')
  if (savedLocale === 'zh' || savedLocale === 'en') {
    return savedLocale
  }
  const browserLang = navigator.language
  const defaultLocale = browserLang.startsWith('zh') ? 'zh' : 'en'
  localStorage.setItem('vust_locale', defaultLocale)
  return defaultLocale
}

const i18n = createI18n({
  legacy: false, // use Composition API
  locale: getInitialLocale(),
  fallbackLocale: 'en',
  messages: {
    en,
    zh,
  },
})

export default i18n
