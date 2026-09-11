import { computed, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'

export type CronMode = 'simple' | 'advanced'
export type SimpleCronType = 'every_minutes' | 'hourly' | 'daily' | 'weekly'

/** 构建严格的五段分钟级 Cron，不接受或生成秒字段。 */
export function useCronBuilder(initialExpr: string = '*/5 * * * *') {
  const { t } = useI18n()

  const cronMode = ref<CronMode>('simple')
  const advancedCronExpr = ref(initialExpr)

  const simpleCron = reactive({
    type: 'every_minutes' as SimpleCronType,
    intervalMinutes: 5,
    minute: 0,
    hour: 0,
    weekday: 1,
  })

  const weekdayOptions = computed(() => [
    { value: 0, label: t('app.taskScheduler.form.weekdays.sun') },
    { value: 1, label: t('app.taskScheduler.form.weekdays.mon') },
    { value: 2, label: t('app.taskScheduler.form.weekdays.tue') },
    { value: 3, label: t('app.taskScheduler.form.weekdays.wed') },
    { value: 4, label: t('app.taskScheduler.form.weekdays.thu') },
    { value: 5, label: t('app.taskScheduler.form.weekdays.fri') },
    { value: 6, label: t('app.taskScheduler.form.weekdays.sat') },
  ])

  const clampInt = (value: number, min: number, max: number) => {
    const n = Math.floor(Number(value) || min)
    return Math.min(max, Math.max(min, n))
  }

  const parseSimpleCron = (expr: string) => {
    const parts = expr.trim().split(/\s+/)
    if (parts.length !== 5) return null
    const min = parts[0] ?? ''
    const hour = parts[1] ?? ''
    const day = parts[2] ?? ''
    const month = parts[3] ?? ''
    const week = parts[4] ?? ''

    if (min.startsWith('*/') && hour === '*' && day === '*' && month === '*' && week === '*') {
      const n = Number(min.slice(2))
      if (!Number.isNaN(n) && n >= 1 && n <= 59) {
        return {
          type: 'every_minutes' as SimpleCronType,
          intervalMinutes: n,
          minute: 0,
          hour: 0,
          weekday: 1,
        }
      }
    }
    if (day === '*' && month === '*' && week === '*' && hour === '*') {
      const m = Number(min)
      if (!Number.isNaN(m) && m >= 0 && m <= 59) {
        return {
          type: 'hourly' as SimpleCronType,
          intervalMinutes: 5,
          minute: m,
          hour: 0,
          weekday: 1,
        }
      }
    }
    if (day === '*' && month === '*' && week === '*') {
      const m = Number(min)
      const h = Number(hour)
      if (!Number.isNaN(m) && !Number.isNaN(h) && m >= 0 && m <= 59 && h >= 0 && h <= 23) {
        return {
          type: 'daily' as SimpleCronType,
          intervalMinutes: 5,
          minute: m,
          hour: h,
          weekday: 1,
        }
      }
    }
    if (day === '*' && month === '*') {
      const m = Number(min)
      const h = Number(hour)
      const w = Number(week)
      if (!Number.isNaN(m) && !Number.isNaN(h) && !Number.isNaN(w)) {
        if (m >= 0 && m <= 59 && h >= 0 && h <= 23 && w >= 0 && w <= 6) {
          return {
            type: 'weekly' as SimpleCronType,
            intervalMinutes: 5,
            minute: m,
            hour: h,
            weekday: w,
          }
        }
      }
    }
    return null
  }

  const cronExprForSubmit = computed(() => {
    if (cronMode.value === 'advanced') {
      return advancedCronExpr.value.trim()
    }
    const min = clampInt(simpleCron.minute, 0, 59)
    const hour = clampInt(simpleCron.hour, 0, 23)
    const weekday = clampInt(simpleCron.weekday, 0, 6)
    const intervalMin = clampInt(simpleCron.intervalMinutes, 1, 59)

    if (simpleCron.type === 'every_minutes') return `*/${intervalMin} * * * *`
    if (simpleCron.type === 'hourly') return `${min} * * * *`
    if (simpleCron.type === 'daily') return `${min} ${hour} * * *`
    return `${min} ${hour} * * ${weekday}`
  })

  const cronSummary = computed(() => {
    const min = clampInt(simpleCron.minute, 0, 59)
    const hour = clampInt(simpleCron.hour, 0, 23)
    const intervalMin = clampInt(simpleCron.intervalMinutes, 1, 59)
    if (simpleCron.type === 'every_minutes') {
      return t('app.taskScheduler.form.summary.everyMinutes', { n: intervalMin })
    }
    if (simpleCron.type === 'hourly') {
      return t('app.taskScheduler.form.summary.hourly', { min })
    }
    if (simpleCron.type === 'daily') {
      return t('app.taskScheduler.form.summary.daily', { hour, min })
    }
    return t('app.taskScheduler.form.summary.weekly', {
      weekday:
        weekdayOptions.value.find((item) => item.value === simpleCron.weekday)?.label ??
        simpleCron.weekday,
      hour,
      min,
    })
  })

  const switchCronMode = (mode: CronMode) => {
    if (mode === cronMode.value) return
    if (mode === 'advanced') {
      advancedCronExpr.value = cronExprForSubmit.value
    }
    cronMode.value = mode
  }

  const loadCron = (expr: string) => {
    const parsed = parseSimpleCron(expr)
    if (parsed) {
      cronMode.value = 'simple'
      simpleCron.type = parsed.type
      simpleCron.intervalMinutes = parsed.intervalMinutes
      simpleCron.minute = parsed.minute
      simpleCron.hour = parsed.hour
      simpleCron.weekday = parsed.weekday
      advancedCronExpr.value = expr
    } else {
      cronMode.value = 'advanced'
      advancedCronExpr.value = expr
    }
  }

  const resetCron = () => {
    cronMode.value = 'simple'
    advancedCronExpr.value = '*/5 * * * *'
    simpleCron.type = 'every_minutes'
    simpleCron.intervalMinutes = 5
    simpleCron.minute = 0
    simpleCron.hour = 0
    simpleCron.weekday = 1
  }

  return {
    cronMode,
    advancedCronExpr,
    simpleCron,
    weekdayOptions,
    cronExprForSubmit,
    cronSummary,
    switchCronMode,
    loadCron,
    resetCron,
  }
}
