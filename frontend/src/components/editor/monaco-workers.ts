/**
 * @file monaco-workers.ts
 * @description 配置 Monaco Editor Web Worker 环境。
 * 使用 Vite 原生的 `?worker` import 方式加载 Worker，兼容 Vite 8+。
 * 必须在首次使用 Monaco API 之前调用 `setupMonacoWorkers()`。
 */

import * as monaco from 'monaco-editor'

import EditorWorker from 'monaco-editor/esm/vs/editor/editor.worker?worker'
import JsonWorker from 'monaco-editor/esm/vs/language/json/json.worker?worker'
import CssWorker from 'monaco-editor/esm/vs/language/css/css.worker?worker'
import HtmlWorker from 'monaco-editor/esm/vs/language/html/html.worker?worker'
import TsWorker from 'monaco-editor/esm/vs/language/typescript/ts.worker?worker'

let initialized = false

/**
 * 初始化 Monaco Editor Worker 环境（幂等调用）
 */
export function setupMonacoWorkers(): void {
  if (initialized) return
  initialized = true

  self.MonacoEnvironment = {
    getWorker(_workerId: string, label: string): Worker {
      if (label === 'json') return new JsonWorker()
      if (label === 'css' || label === 'scss' || label === 'less') return new CssWorker()
      if (label === 'html' || label === 'handlebars' || label === 'razor') return new HtmlWorker()
      if (label === 'typescript' || label === 'javascript') return new TsWorker()
      return new EditorWorker()
    },
  }
}

export { monaco }
