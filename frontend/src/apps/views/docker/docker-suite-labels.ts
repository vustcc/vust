/**
 * @file docker-suite-labels.ts
 * @description Docker 资源套件归属标签识别工具。
 */

export type DockerResourceLabels = Record<string, string> | null | undefined

export function isSuiteManagedResource(labels: DockerResourceLabels): boolean {
  if (!labels) return false
  return labels['vust.owner'] === 'suite'
}
