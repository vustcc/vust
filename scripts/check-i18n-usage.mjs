import fs from 'node:fs'
import path from 'node:path'
import ts from '../frontend/node_modules/typescript/lib/typescript.js'

const repositoryRoot = path.resolve(import.meta.dirname, '..')

function localeKeys(file) {
  const source = ts.createSourceFile(
    file,
    fs.readFileSync(file, 'utf8'),
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TS,
  )
  const keys = new Set()
  const visitObject = (node, prefix = '') => {
    for (const property of node.properties) {
      if (!ts.isPropertyAssignment(property)) continue
      const name = property.name.getText(source).replace(/^['"]|['"]$/g, '')
      const key = prefix ? `${prefix}.${name}` : name
      if (ts.isObjectLiteralExpression(property.initializer)) visitObject(property.initializer, key)
      else keys.add(key)
    }
  }
  for (const statement of source.statements) {
    if (ts.isExportAssignment(statement) && ts.isObjectLiteralExpression(statement.expression)) {
      visitObject(statement.expression)
    }
    if (ts.isVariableStatement(statement)) {
      for (const declaration of statement.declarationList.declarations) {
        if (declaration.initializer && ts.isObjectLiteralExpression(declaration.initializer)) {
          visitObject(declaration.initializer)
        }
      }
    }
  }
  return keys
}

const zh = localeKeys(path.join(repositoryRoot, 'frontend/src/locales/zh.ts'))
const en = localeKeys(path.join(repositoryRoot, 'frontend/src/locales/en.ts'))
const onlyZh = [...zh].filter((key) => !en.has(key))
const onlyEn = [...en].filter((key) => !zh.has(key))

const loggingSource = fs.readFileSync(
  path.join(repositoryRoot, 'crates/vust/src/services/logging.rs'),
  'utf8',
)
const registry = loggingSource.slice(
  loggingSource.indexOf('const REGISTERED_EVENT_GROUPS'),
  loggingSource.indexOf('fn registered_event'),
)
const registeredEvents = [...registry.matchAll(/"([a-z][a-z0-9_]+)"/g)].map(
  (match) => match[1],
)
const missingEvents = registeredEvents.filter(
  (code) =>
    !zh.has(`app.operationLog.events.${code}`) || !en.has(`app.operationLog.events.${code}`),
)

if (onlyZh.length || onlyEn.length || missingEvents.length) {
  if (onlyZh.length) console.error(`Only in zh: ${onlyZh.join(', ')}`)
  if (onlyEn.length) console.error(`Only in en: ${onlyEn.join(', ')}`)
  if (missingEvents.length) {
    console.error(`Missing operation event translations: ${missingEvents.join(', ')}`)
  }
  process.exitCode = 1
}
