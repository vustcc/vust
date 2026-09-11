/** Docker 资源趋势在当前浏览器中的偏好存储键。 */
export const DOCKER_OVERVIEW_PREFERENCES_STORAGE_KEY = 'vust_docker_overview_preferences'

/** Docker 资源趋势最多可同时选择的容器数。 */
export const MAX_OVERVIEW_CONTAINERS = 7

/** 用于跨容器重建恢复选择的轻量引用。 */
export interface DockerOverviewContainerReference {
  id: string
  name: string
}

interface DockerOverviewPreferences {
  version: 1
  selectionsByNode: Record<string, DockerOverviewContainerReference[]>
}

const createEmptyPreferences = (): DockerOverviewPreferences => ({
  version: 1,
  selectionsByNode: {},
})

/** 校验、去重并限制持久化的容器引用。 */
function normalizeContainerReferences(value: unknown): DockerOverviewContainerReference[] | null {
  if (!Array.isArray(value)) return null

  const references: DockerOverviewContainerReference[] = []
  const ids = new Set<string>()
  for (const item of value) {
    if (!item || typeof item !== 'object') continue
    const id = 'id' in item && typeof item.id === 'string' ? item.id.trim() : ''
    const name = 'name' in item && typeof item.name === 'string' ? item.name.trim() : ''
    if (!id || !name || ids.has(id)) continue
    ids.add(id)
    references.push({ id, name })
    if (references.length === MAX_OVERVIEW_CONTAINERS) break
  }
  return references
}

/** 安全读取完整偏好；损坏或不兼容数据会降级为空配置。 */
function readPreferences(): DockerOverviewPreferences {
  if (typeof window === 'undefined') return createEmptyPreferences()
  try {
    const raw = window.localStorage.getItem(DOCKER_OVERVIEW_PREFERENCES_STORAGE_KEY)
    if (!raw) return createEmptyPreferences()
    const parsed: unknown = JSON.parse(raw)
    if (
      !parsed ||
      typeof parsed !== 'object' ||
      !('version' in parsed) ||
      parsed.version !== 1 ||
      !('selectionsByNode' in parsed) ||
      !parsed.selectionsByNode ||
      typeof parsed.selectionsByNode !== 'object' ||
      Array.isArray(parsed.selectionsByNode)
    ) {
      return createEmptyPreferences()
    }
    return {
      version: 1,
      selectionsByNode: parsed.selectionsByNode as Record<
        string,
        DockerOverviewContainerReference[]
      >,
    }
  } catch {
    return createEmptyPreferences()
  }
}

/** 读取指定节点的容器选择；`null` 表示该节点尚无历史配置。 */
export function readDockerOverviewSelection(
  nodeId: string,
): DockerOverviewContainerReference[] | null {
  const normalizedNodeId = nodeId.trim()
  if (!normalizedNodeId) return null

  const preferences = readPreferences()
  if (!Object.prototype.hasOwnProperty.call(preferences.selectionsByNode, normalizedNodeId)) {
    return null
  }
  const storedSelection = preferences.selectionsByNode[normalizedNodeId]
  const normalizedSelection = normalizeContainerReferences(storedSelection)
  if (
    normalizedSelection === null ||
    (storedSelection.length > 0 && normalizedSelection.length === 0)
  ) {
    return null
  }
  return normalizedSelection
}

/** 按节点写入容器选择，并保留其他节点的偏好。 */
export function writeDockerOverviewSelection(
  nodeId: string,
  references: DockerOverviewContainerReference[],
): boolean {
  const normalizedNodeId = nodeId.trim()
  const normalizedReferences = normalizeContainerReferences(references)
  if (!normalizedNodeId || normalizedReferences === null || typeof window === 'undefined') {
    return false
  }

  try {
    const preferences = readPreferences()
    preferences.selectionsByNode[normalizedNodeId] = normalizedReferences
    window.localStorage.setItem(
      DOCKER_OVERVIEW_PREFERENCES_STORAGE_KEY,
      JSON.stringify(preferences),
    )
    return true
  } catch {
    return false
  }
}

/** 将当前容器 ID 转换为可持久化引用。 */
export function buildDockerOverviewContainerReferences(
  items: DockerOverviewContainerReference[],
  selectedIds: string[],
): DockerOverviewContainerReference[] {
  const itemById = new Map(items.map((item) => [item.id, item]))
  return selectedIds
    .map((id) => itemById.get(id))
    .filter((item): item is DockerOverviewContainerReference => Boolean(item))
    .slice(0, MAX_OVERVIEW_CONTAINERS)
    .map(({ id, name }) => ({ id, name }))
}

/** 优先按 ID、再按完整名称恢复当前节点仍然存在的容器。 */
export function resolveDockerOverviewContainerIds(
  items: DockerOverviewContainerReference[],
  references: DockerOverviewContainerReference[],
): string[] {
  const itemById = new Map(items.map((item) => [item.id, item]))
  const itemByName = new Map(items.map((item) => [item.name, item]))
  const resolvedIds: string[] = []
  const seen = new Set<string>()

  for (const reference of references) {
    const item = itemById.get(reference.id) || itemByName.get(reference.name)
    if (!item || seen.has(item.id)) continue
    seen.add(item.id)
    resolvedIds.push(item.id)
    if (resolvedIds.length === MAX_OVERVIEW_CONTAINERS) break
  }
  return resolvedIds
}
