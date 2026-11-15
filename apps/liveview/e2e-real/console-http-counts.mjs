import {openSync, closeSync, readFileSync, fstatSync, constants} from 'node:fs'

const fields = ['version', 'cap', 'total', 'gatewayPost', 'authPreflight', 'otherServices', 'other']
const families = fields.slice(3)

function validate(counts) {
  if (!counts || Object.keys(counts).sort().join() !== [...fields].sort().join() ||
      counts.version !== 1 || counts.cap !== 2048 ||
      fields.some(field => !Number.isSafeInteger(counts[field]) || counts[field] < 0) ||
      counts.total > counts.cap || families.reduce((sum, field) => sum + counts[field], 0) !== counts.total) {
    throw Error('PRIVATE_HTTP_ACCOUNTING_INVALID')
  }
  return counts
}

export function readConsoleHttpCounts(path) {
  let descriptor
  try {
    descriptor = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW)
    const stat = fstatSync(descriptor)
    if (!stat.isFile() || stat.size > 4096 || (stat.mode & 0o077) !== 0) throw Error()
    return validate(JSON.parse(readFileSync(descriptor, 'utf8')))
  } catch {
    throw Error('PRIVATE_HTTP_ACCOUNTING_INVALID')
  } finally {
    if (descriptor !== undefined) closeSync(descriptor)
  }
}

export function consoleHttpDelta(before, after, direct) {
  validate(before)
  validate(after)
  if (!Number.isSafeInteger(direct) || direct < 0 ||
      ['total', ...families].some(field => after[field] < before[field])) throw Error('PRIVATE_HTTP_ACCOUNTING_INVALID')
  return {
    gatewayUiActual: after.gatewayPost - before.gatewayPost,
    gatewayActualCombined: after.gatewayPost - before.gatewayPost + direct,
    consoleHttpActual: after.total - before.total,
    authPreflightActual: after.authPreflight - before.authPreflight,
    otherServicesActual: after.otherServices - before.otherServices,
    otherHttpActual: after.other - before.other,
    consoleHttpBaseline: before.total,
    consoleHttpFinal: after.total,
    consoleHttpHardCap: 2048,
  }
}
