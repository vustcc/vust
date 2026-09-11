const staticAssetVersion = `${import.meta.env.VITE_APP_VERSION ?? '0.0.0'}-${(
  import.meta.env.VITE_GIT_HASH ?? 'unknown'
).slice(0, 8)}`

/**
 * @description 为稳定文件名的内置静态资源追加构建版本，使浏览器缓存可在新构建后失效。
 */
export function versionStaticAsset(path: string) {
  const separator = path.includes('?') ? '&' : '?'
  return `${path}${separator}v=${encodeURIComponent(staticAssetVersion)}`
}
