import http from '@/api'

export interface SecuritySettings {
  safeEntry: string
  safeEntryEnabled: boolean
  passwordComplexity: boolean
}

export interface UpdateSecuritySettingsPayload {
  safeEntry: string
  passwordComplexity: boolean
}

export const securityApi = {
  fetchSettings: () => http.get<SecuritySettings>('/security/settings'),
  updateSettings: (payload: UpdateSecuritySettingsPayload) =>
    http.put<void>('/security/settings', payload),
  updatePassword: (newPassword: string) => http.put<void>('/security/password', { newPassword }),
  updateUsername: (newUsername: string) => http.put<void>('/security/username', { newUsername }),
}
