const LOGIN_ENTRY_STORAGE_KEY = 'vust.loginEntryPath'

export function isSafeEntryPath(path: string): boolean {
  return /^\/[A-Za-z0-9]{8,32}$/.test(path)
}

export function rememberLoginEntry(path: string) {
  if (isSafeEntryPath(path)) {
    window.sessionStorage.setItem(LOGIN_ENTRY_STORAGE_KEY, path)
  }
}

export function forgetLoginEntry() {
  window.sessionStorage.removeItem(LOGIN_ENTRY_STORAGE_KEY)
}

export function getLoginEntryPath(): string {
  const stored = window.sessionStorage.getItem(LOGIN_ENTRY_STORAGE_KEY)
  if (stored && isSafeEntryPath(stored)) {
    return stored
  }
  if (isSafeEntryPath(window.location.pathname)) {
    return window.location.pathname
  }
  return '/login'
}
