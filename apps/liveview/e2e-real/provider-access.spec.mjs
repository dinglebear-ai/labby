import {test, expect} from '@playwright/test'
import {randomBytes} from 'node:crypto'
import {writeFileSync} from 'node:fs'
import {readProviderAccessManifest} from './provider-run-output.mjs'
import {teamInventoryRow, withInvitationClipboard} from './provider-access-checks.mjs'
import {providerSignIn} from './provider-browser.mjs'
import {validateProofSelection} from './provider-run-ownership.mjs'
import {readConsoleHttpCounts, consoleHttpDelta} from './console-http-counts.mjs'

// Literal acceptance contract. In particular, a 21-action binary must fail
// admission before an unavailable activate/unassign control can hide a gap.
const writes = [
  'access.team.create', 'access.team.member.add', 'access.team.member.role.set',
  'access.team.member.suspend', 'access.team.member.remove', 'access.team.member.activate',
  'access.team.suspend', 'access.team.activate', 'access.team_invitation.create',
  'access.team_invitation.accept', 'access.team_project.assign', 'access.team_project.unassign',
  'access.platform_admin.grant', 'access.platform_admin.revoke',
  'access.gateway_credential.bind', 'access.gateway_credential.revoke',
]
const reads = [
  'access.team.list', 'access.team.member.list', 'access.team_project.list',
  'access.team_invitation.list', 'access.platform_admin.list',
  'access.project.effective.list', 'access.gateway_credential.list',
]
const cards = ['teams', 'members', 'assignments', 'invitations', 'admins', 'effective', 'credentials']
const consoleOrigin = 'http://127.0.0.1:4328'
const allowedOrigins = new Set([consoleOrigin, 'http://127.0.0.1:4329', 'https://127.0.0.1:4331'])
const actionButton = (page, action) => page.locator(`#access-action-${action.replaceAll('.', '-')}`)
const card = (page, name) => page.locator(`#access-card-${name}`)
const row = (page, name, id) => card(page, name).locator('article').filter({hasText: id})

function ownedManifest(info) {
  const use = info.project.use
  if (!process.env.PLAYWRIGHT_NO_COPY_PROMPT || use.trace !== 'off' ||
      use.screenshot !== 'off' || use.video !== 'off') throw Error('ACCESS_PRIVATE_ARTIFACT_GUARD_REQUIRED')
  const runId = process.env.LABBY_E2E_PROVIDER_RUN_ID
  if (!/^[a-f0-9]{32}$/.test(runId || '')) throw Error('ACCESS_RUN_ID_REQUIRED')
  const manifest = readProviderAccessManifest()
  const selected = validateProofSelection({
    binary: process.env.LABBY_E2E_BINARY,
    expectedSha256: process.env.LABBY_E2E_EXPECTED_BINARY_SHA256,
    sourceRevision: process.env.LABBY_E2E_SOURCE_REVISION,
  })
  if (manifest.runId !== runId || manifest.binarySha256 !== selected.binarySha256 || manifest.sourceRevision !== selected.sourceRevision ||
      manifest.binary !== selected.binary || !manifest.consoleHttpReceipt ||
      !manifest.ownerPrincipalId || !manifest.memberPrincipalId ||
      manifest.ownerPrincipalId === manifest.memberPrincipalId) throw Error('ACCESS_CANDIDATE_IDENTITY_MISMATCH')
  const actual = manifest.accessActions
  if (!Array.isArray(actual) || new Set(actual).size !== actual.length ||
      actual.length !== 23 || [...writes, ...reads].some(action => !actual.includes(action))) {
    throw Error('ACCESS_23_ACTION_PREREQUISITE_FAILED')
  }
  return manifest
}

async function inventory(page, name, state = 'loaded') {
  await expect(card(page, name)).toHaveAttribute('data-state', state)
}

async function refresh(page, name, state = 'loaded') {
  const disclosure = name === 'teams' ? page.locator('#access-teams-inventory') :
    name === 'effective' ? page.locator('.ac-effective') : null
  if (disclosure && await disclosure.getAttribute('open') === null) await disclosure.locator('summary').click()
  const button = page.locator(`#access-refresh-${name}`)
  await button.click()
  await expect(button).not.toHaveClass(/phx-click-loading/)
  await inventory(page, name, state)
}

async function selectTeam(page, team) {
  await page.locator('#access-team').selectOption(team)
  await expect(page.locator('#access-team-form')).not.toHaveClass(/phx-change-loading/)
  await expect(page.locator('#access-team')).toHaveValue(team)
  for (const name of ['members', 'assignments', 'credentials']) await inventory(page, name)
}

async function openAction(page, action) {
  const menu = page.locator('#access-actions')
  if (await menu.getAttribute('open') === null) await menu.locator('summary').click()
  await expect(actionButton(page, action)).toBeEnabled()
  await actionButton(page, action).click()
  await expect(page.locator('#access-action-form')).toHaveAttribute('data-action', action)
}

async function apply(page, action, fields, status = 'reconciled') {
  await openAction(page, action)
  for (const [field, value] of Object.entries(fields)) {
    const control = page.locator(`#access-field-${field}`)
    if (field === 'role') await control.selectOption(value)
    else await control.fill(value)
  }
  await page.locator('#access-confirm').check()
  // One submission only. Read-back may poll, but a failed mutation is never replayed.
  await page.locator('#access-submit').click()
  await expect(page.locator('#access-dialog')).toHaveCount(0)
  await expect(page.locator('#access-receipt')).toHaveAttribute('data-status', status)
}

async function assertMember(page, principal, role, status) {
  const member = row(page, 'members', principal)
  await expect(member).toHaveCount(1)
  await expect(member.locator('.ac-record > span').first()).toHaveText(role)
  await expect(member.locator('[data-status]')).toHaveAttribute('data-status', status)
}

async function revision(page, name) {
  const text = await card(page, name).locator(':scope > .ac-meta').textContent()
  const value = text.match(/Revision (\d+)/)?.[1]
  if (!value) throw Error('ACCESS_INVENTORY_REVISION_REQUIRED')
  return Number(value)
}

async function reloadTeam(page, team) {
  await page.reload()
  await inventory(page, 'teams')
  await selectTeam(page, team)
}

async function scenario(browser, info, run) {
  test.setTimeout(180000)
  let stage = 'private-admission'
  let failure
  const contexts = []
  const receipt = {ownedTeams: [], ownedTeamNames: {}, ownedProjects: [], ownedReferences: [], actions: [],
    browserRequestStarts: 0, browserRequestCap: 512, browserErrors: 0, cleanup: 'not-started'}
  let manifest
  let baseline
  const step = async (label, operation) => {
    stage = label
    return operation()
  }
  try {
    manifest = ownedManifest(info)
    baseline = readConsoleHttpCounts(manifest.consoleHttpReceipt)
    receipt.binarySha256 = manifest.binarySha256
    receipt.sourceRevision = manifest.sourceRevision
    receipt.fixture = 'public-OIDC-provisioned owner/member and pre-existing provider Team/Project'
    const ownerContext = await browser.newContext({baseURL: consoleOrigin, viewport: {width: 1440, height: 1000}})
    contexts.push(ownerContext)
    const memberContext = await browser.newContext({baseURL: consoleOrigin, viewport: {width: 1440, height: 1000}})
    contexts.push(memberContext)
    for (const context of contexts) {
      context.on('request', request => {
        receipt.browserRequestStarts++
        if (!allowedOrigins.has(new URL(request.url()).origin) || receipt.browserRequestStarts > 512) {
          failure ||= Error('ACCESS_BROWSER_REQUEST_BUDGET_OR_ORIGIN_FAILED')
          for (const owned of contexts) void owned.setOffline(true).catch(() => {})
        }
      })
      context.on('page', page => {
        page.setDefaultTimeout(10000)
        page.setDefaultNavigationTimeout(15000)
        page.on('pageerror', () => receipt.browserErrors++)
      })
    }
    const owner = await ownerContext.newPage()
    const member = await memberContext.newPage()
    await step('owner-provider-signin', () => providerSignIn(owner, {
      email: 'owner@controlled.invalid', password: process.env.LABBY_E2E_PROVIDER_PASSWORD,
    }))
    await step('member-provider-signin', () => providerSignIn(member, {
      email: 'member@controlled.invalid', password: process.env.LABBY_E2E_MEMBER_PASSWORD,
    }))
    await step('normal-access-entry', async () => {
      await owner.goto(`${consoleOrigin}/access`)
      for (const name of cards) await inventory(owner, name)
      await member.goto(`${consoleOrigin}/access`)
      await inventory(member, 'teams')
      await inventory(member, 'admins', 'forbidden')
      for (const action of writes) await expect(actionButton(owner, action)).toBeEnabled()
    })
    const change = async (page, action, fields, status) => {
      if (receipt.actions.length >= 64) throw Error('ACCESS_MUTATION_ADMISSION_BUDGET')
      receipt.actions.push({action, expectedStatus: status || 'reconciled'})
      return apply(page, action, fields, status)
    }
    const createTeam = async label => {
      const id = `access-${label}-${randomBytes(6).toString('hex')}`
      const name = `Acceptance ${label}-${id.slice(-12)}`
      receipt.ownedTeams.push(id)
      receipt.ownedTeamNames[id] = name
      await change(owner, 'access.team.create', {team_id: id, name})
      await selectTeam(owner, id)
      await assertMember(owner, manifest.ownerPrincipalId, 'owner', 'active')
      return id
    }
    await run({owner, member, manifest, receipt, step, change, createTeam})
    await step('final-browser-health', async () => {
      expect(receipt.browserErrors).toBe(0)
      expect(receipt.browserRequestStarts).toBeLessThanOrEqual(512)
    })
  } catch (error) {
    // Values filled into token/password fields must never reach runner diagnostics.
    const code = /^ACCESS_[A-Z0-9_]+$/.test(error?.message || '') ? error.message :
      error?.name === 'TimeoutError' ? 'timeout' : 'assertion'
    failure ||= Error(`PRIVATE_ACCESS_FAILED stage=${stage} kind=${code}`)
  } finally {
    stage = 'owned-context-cleanup'
    const closed = await Promise.allSettled(contexts.map(context => context.close()))
    receipt.cleanup = closed.every(result => result.status === 'fulfilled') ? 'contexts-closed' : 'unconfirmed'
    receipt.domainRetention = 'owned ephemeral rows and audit history retained until provider harness shutdown; no hard-delete API'
    try {
      if (manifest && baseline) {
        Object.assign(receipt, consoleHttpDelta(baseline, readConsoleHttpCounts(manifest.consoleHttpReceipt), 0))
        // The BEAM guard admits at most 2048 Finch starts for the whole cohort.
        // These deltas count request starts, not hidden transport retries.
        if (receipt.consoleHttpActual > 600) failure ||= Error('ACCESS_CONSOLE_HTTP_BUDGET_EXCEEDED')
      }
      const path = info.outputPath('private-access-receipt.json')
      writeFileSync(path, JSON.stringify(receipt, null, 2), {mode: 0o600})
      await info.attach('private-access-receipt', {path, contentType: 'application/json'})
    } catch {
      failure ||= Error('ACCESS_PRIVATE_RECEIPT_FAILED')
    }
    if (receipt.cleanup !== 'contexts-closed') failure ||= Error('ACCESS_CONTEXT_CLEANUP_FAILED')
  }
  if (failure) throw failure
}

test('native Access persists lifecycle, protects the last owner, clears revoked authority and rotates reference generations', async ({browser}, info) => {
  await scenario(browser, info, async ({owner, member, manifest, receipt, step, change, createTeam}) => {
    const team = await step('create-owned-lifecycle-team', () => createTeam(`lifecycle-${'long-id-'.repeat(8)}`))
    const memberId = manifest.memberPrincipalId
    await step('last-owner-conflicts-have-zero-visible-effects', async () => {
      const before = await revision(owner, 'members')
      for (const action of ['access.team.member.remove', 'access.team.member.suspend', 'access.team.member.role.set']) {
        await change(owner, action, {principal_id: manifest.ownerPrincipalId,
          ...(action.endsWith('role.set') ? {role: 'member'} : {})}, 'failed')
        await expect(owner.locator('#access-receipt')).toContainText('last owner is protected')
        await refresh(owner, 'members')
        await assertMember(owner, manifest.ownerPrincipalId, 'owner', 'active')
        expect(await revision(owner, 'members')).toBe(before)
      }
      receipt.lastOwnerRevision = before
    })
    await step('member-add-and-current-authority', async () => {
      await change(owner, 'access.team.member.add', {principal_id: memberId, role: 'admin'})
      await assertMember(owner, memberId, 'admin', 'active')
      await refresh(member, 'teams')
      await selectTeam(member, team)
      await expect(actionButton(member, 'access.team.member.add')).toBeEnabled()
    })
    await step('suspend-role-change-and-explicit-restore', async () => {
      await change(owner, 'access.team.member.suspend', {principal_id: memberId})
      await assertMember(owner, memberId, 'admin', 'suspended')
      await change(owner, 'access.team.member.role.set', {principal_id: memberId, role: 'member'})
      await assertMember(owner, memberId, 'member', 'suspended')
      await change(owner, 'access.team.member.activate', {principal_id: memberId, role: 'admin'})
      await assertMember(owner, memberId, 'admin', 'active')
      await reloadTeam(owner, team)
      await assertMember(owner, memberId, 'admin', 'active')
    })
    await step('connected-revocation-clears-private-roster', async () => {
      await change(owner, 'access.team.member.remove', {principal_id: memberId})
      await assertMember(owner, memberId, 'admin', 'revoked')
      await refresh(member, 'members', 'forbidden')
      await expect(card(member, 'members').locator('article')).toHaveCount(0)
      await refresh(member, 'teams')
      await expect(member.locator(`#access-team option[value="${team}"]`)).toHaveCount(0)
      await member.goto(`${consoleOrigin}/projects`)
      await expect(member.locator('[data-phx-main]')).toHaveClass(/phx-connected/)
      await change(owner, 'access.team.member.activate', {principal_id: memberId, role: 'admin'})
      await assertMember(owner, memberId, 'admin', 'active')
      await member.goto(`${consoleOrigin}/access`)
      await inventory(member, 'teams')
      await selectTeam(member, team)
    })
    await step('team-suspend-and-explicit-activate', async () => {
      await expect(owner.locator('#access-team')).toHaveValue(team)
      await change(owner, 'access.team.suspend', {})
      await expect(teamInventoryRow(owner, {id: team, name: receipt.ownedTeamNames[team]}).locator('[data-status]')).toHaveAttribute('data-status', 'suspended')
      await refresh(member, 'teams')
      await expect(actionButton(member, 'access.team.member.add')).toBeDisabled()
      await change(owner, 'access.team.activate', {})
      await reloadTeam(owner, team)
      await expect(owner.locator('#access-team')).toHaveValue(team)
      await expect(teamInventoryRow(owner, {id: team, name: receipt.ownedTeamNames[team]}).locator('[data-status]')).toHaveAttribute('data-status', 'active')
    })
    await step('reference-bind-replace-revoke-readback', async () => {
      const upstream = 'provider-real-mcp'
      const first = `owned-reference-${randomBytes(6).toString('hex')}`
      const second = `${first}-replacement`
      receipt.ownedReferences.push({team, upstream, references: [first, second]})
      await change(owner, 'access.gateway_credential.bind', {upstream_name: upstream, binding_id: first})
      const binding = row(owner, 'credentials', upstream)
      await expect(binding).toContainText(`Reference ${first}`)
      await expect(binding).toContainText(`Custodian ${manifest.ownerPrincipalId} · Generation 1`)
      await change(owner, 'access.gateway_credential.bind', {upstream_name: upstream, binding_id: second})
      await expect(binding).toContainText(`Reference ${second}`)
      await expect(binding).toContainText('Generation 2')
      await reloadTeam(owner, team)
      await expect(binding).toContainText(`Reference ${second}`)
      await expect(binding).toContainText(`Custodian ${manifest.ownerPrincipalId} · Generation 2`)
      await change(owner, 'access.gateway_credential.revoke', {upstream_name: upstream})
      await expect(binding.locator('[data-status]')).toHaveAttribute('data-status', 'revoked')
      await expect(binding).toContainText('Generation 3')
      await reloadTeam(owner, team)
      await expect(binding.locator('[data-status]')).toHaveAttribute('data-status', 'revoked')
      receipt.credentialProof = 'durable nonsecret reference/custodian/generation only; no upstream credential session established'
    })
    await step('platform-grant-revoke-and-authoritative-clear', async () => {
      await change(owner, 'access.platform_admin.grant', {principal_id: memberId})
      await expect(row(owner, 'admins', memberId).locator('[data-status]')).toHaveAttribute('data-status', 'active')
      await member.reload()
      await inventory(member, 'admins')
      await expect(actionButton(member, 'access.team.create')).toBeEnabled()
      await change(owner, 'access.platform_admin.revoke', {principal_id: memberId})
      await expect(row(owner, 'admins', memberId).locator('[data-status]')).toHaveAttribute('data-status', 'revoked')
      await refresh(member, 'admins', 'forbidden')
      await expect(card(member, 'admins').locator('article')).toHaveCount(0)
      await expect(actionButton(member, 'access.platform_admin.grant')).toBeDisabled()
      await member.reload()
      await inventory(member, 'admins', 'forbidden')
      await expect(actionButton(member, 'access.team.create')).toBeDisabled()
    })
    await step('responsive-navigation-and-keyboard-dialog', async () => {
      for (const width of [1440, 740, 320]) {
        await owner.setViewportSize({width, height: 1000})
        expect(await owner.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
        for (const panel of await owner.locator('.ac-main .ac-panel').all()) {
          const box = await panel.boundingBox()
          if (box) expect(box.x >= 0 && box.x + box.width <= width).toBe(true)
        }
        const menu = owner.locator('#access-actions')
        if (await menu.getAttribute('open') !== null) await menu.locator('summary').click()
        await menu.locator('summary').focus()
        await owner.keyboard.press('Enter')
        await expect(menu).toHaveAttribute('open', '')
        await owner.keyboard.press('Escape')
        await expect(menu).not.toHaveAttribute('open', '')
        await expect(menu.locator('summary')).toBeFocused()
        await openAction(owner, 'access.team.member.add')
        await owner.locator('#access-field-principal_id').fill(memberId)
        await owner.keyboard.press('Escape')
        await expect(owner.locator('#access-dialog')).toHaveCount(0)
        await expect(menu.locator('summary')).toBeFocused()
      }
    })
  })
})

test('native Project assignment requires both managers and rechecks lost sole management before reassigning', async ({browser}, info) => {
  await scenario(browser, info, async ({owner, member, manifest, receipt, step, change, createTeam}) => {
    const projectTeam = await step('create-project-owning-team', () => createTeam('project-owner'))
    const delegatedTeam = await step('create-delegated-team', () => createTeam('delegated'))
    const project = `access-project-${randomBytes(6).toString('hex')}`
    receipt.ownedProjects.push({team: projectTeam, project})
    await step('normal-project-create', async () => {
      await owner.goto(`${consoleOrigin}/projects`)
      await expect(owner.locator('#projects-create')).toBeEnabled()
      await owner.locator('#projects-filter-sheet-toggle').click()
      await owner.locator('#projects-workspace-team').selectOption(projectTeam)
      await owner.getByRole('button', {name: 'Close filters', exact: true}).click()
      await owner.locator('#projects-create').click()
      await expect(owner.locator('input[name="project[team_id]"]')).toHaveValue(projectTeam)
      await owner.locator('#projects-name').fill('Owned native Access project')
      await owner.locator('#projects-id').fill(project)
      await owner.locator('#projects-save').click()
      await expect(owner.locator(`article[data-project="${project}"]`)).toContainText('Owned native Access project')
      await owner.goto(`${consoleOrigin}/access`)
      await inventory(owner, 'teams')
      await selectTeam(owner, delegatedTeam)
      await change(owner, 'access.team.member.add', {principal_id: manifest.memberPrincipalId, role: 'admin'})
      await refresh(member, 'teams')
      await selectTeam(member, delegatedTeam)
    })
    await step('team-manager-without-project-authority-is-denied', async () => {
      await refresh(owner, 'assignments')
      const before = await revision(owner, 'assignments')
      await change(member, 'access.team_project.assign', {project_id: project, role: 'admin'}, 'failed')
      await expect(member.locator('#access-receipt')).toContainText('Access denied')
      await refresh(owner, 'assignments')
      expect(await revision(owner, 'assignments')).toBe(before)
      await expect(row(owner, 'assignments', project)).toHaveCount(0)
    })
    await step('viewer-does-not-confer-project-management', async () => {
      await change(owner, 'access.team_project.assign', {project_id: project, role: 'viewer'})
      await refresh(member, 'assignments')
      await expect(row(member, 'assignments', project)).toContainText('viewer')
      await change(member, 'access.team_project.unassign', {project_id: project}, 'failed')
      await expect(member.locator('#access-receipt')).toContainText('Access denied')
      await refresh(owner, 'assignments')
      await expect(row(owner, 'assignments', project).locator('[data-status]')).toHaveAttribute('data-status', 'active')
    })
    await step('dual-manager-unassign-loses-sole-project-authority', async () => {
      await change(owner, 'access.team_project.assign', {project_id: project, role: 'admin'})
      await reloadTeam(member, delegatedTeam)
      const before = await row(member, 'assignments', project).textContent()
      await change(member, 'access.team_project.unassign', {project_id: project})
      await expect(row(member, 'assignments', project).locator('[data-status]')).toHaveAttribute('data-status', 'revoked')
      await expect(row(member, 'effective', project)).toHaveCount(0)
      const revoked = await row(member, 'assignments', project).textContent()
      expect(revoked).not.toBe(before)
      await reloadTeam(member, delegatedTeam)
      await expect(row(member, 'assignments', project)).toHaveText(revoked)
      await change(member, 'access.team_project.assign', {project_id: project, role: 'admin'}, 'failed')
      await expect(member.locator('#access-receipt')).toContainText('Access denied')
      await refresh(member, 'assignments')
      await expect(row(member, 'assignments', project)).toHaveText(revoked)
    })
    await step('retained-platform-owner-restores-and-persists-assignment', async () => {
      await change(owner, 'access.team_project.assign', {project_id: project, role: 'admin'})
      await reloadTeam(member, delegatedTeam)
      await expect(row(member, 'assignments', project).locator('[data-status]')).toHaveAttribute('data-status', 'active')
      await refresh(member, 'effective')
      await expect(row(member, 'effective', project)).toContainText('admin')
      await change(member, 'access.team_project.assign', {project_id: project, role: 'viewer'})
      await expect(row(member, 'assignments', project)).toContainText('viewer')
      await refresh(owner, 'assignments')
      await change(owner, 'access.team_project.unassign', {project_id: project})
      await reloadTeam(owner, delegatedTeam)
      await expect(row(owner, 'assignments', project).locator('[data-status]')).toHaveAttribute('data-status', 'revoked')
    })
  })
})

test('native principal invitations preserve token custody, cross-Team acceptance, expiry and stale-epoch denials', async ({browser}, info) => {
  await scenario(browser, info, async ({owner, member, manifest, receipt, step, change, createTeam}) => {
    let token
    const issue = async ttl => {
      await openAction(owner, 'access.team_invitation.create')
      await owner.locator('#access-field-principal_id').fill(manifest.memberPrincipalId)
      await owner.locator('#access-field-ttl_seconds').fill(String(ttl))
      await owner.locator('[data-access-generate]').click()
      token = await owner.locator('#access-field-token').inputValue()
      expect(/^[0-9a-f]{64}$/.test(token)).toBe(true)
      expect(await owner.locator('#access-field-token').getAttribute('name')).toBeNull()
      await owner.locator('#access-confirm').check()
      if (receipt.actions.length >= 64) throw Error('ACCESS_MUTATION_ADMISSION_BUDGET')
      receipt.actions.push({action: 'access.team_invitation.create', expectedStatus: 'accepted_observed'})
      await owner.locator('#access-submit').click()
      await expect(owner.locator('#access-receipt')).toHaveAttribute('data-status', 'accepted_observed')
      await expect(owner.locator('#access-one-time')).toBeVisible()
      expect(await owner.evaluate(value => !location.href.includes(value) &&
        !document.documentElement.outerHTML.includes(value) && !document.cookie.includes(value) &&
        ![...Object.values(localStorage), ...Object.values(sessionStorage)].some(stored => stored.includes(value)), token)).toBe(true)
    }
    const team = await step('create-cross-team-invitation', () => createTeam('invite'))
    await step('client-generated-token-and-real-clipboard', async () => {
      await issue(60)
      await owner.locator('[data-access-reveal]').click()
      await expect(owner.locator('[data-access-reveal]')).toBeDisabled()
      await owner.context().grantPermissions(['clipboard-read', 'clipboard-write'], {origin: consoleOrigin})
      await withInvitationClipboard(owner, token, receipt, async () => {
        await owner.locator('[data-access-copy]').click()
        await expect(owner.locator('[data-access-copy-status]')).toContainText('Token copied')
        expect(await owner.evaluate(async value => (await navigator.clipboard.readText()) === value, token)).toBe(true)
        expect(await owner.locator('#access-created-secret').inputValue() === '').toBe(true)
      })
    })
    await step('different-caller-required-and-no-membership-on-denial', async () => {
      await refresh(owner, 'members')
      const before = await revision(owner, 'members')
      await change(owner, 'access.team_invitation.accept', {token}, 'failed')
      await expect(owner.locator('#access-receipt')).toContainText('Access denied')
      await refresh(owner, 'members')
      expect(await revision(owner, 'members')).toBe(before)
      await expect(row(owner, 'members', manifest.memberPrincipalId)).toHaveCount(0)
      await refresh(owner, 'invitations')
      await expect(row(owner, 'invitations', manifest.memberPrincipalId).locator('[data-status]')).toHaveAttribute('data-status', 'pending')
    })
    await step('cross-team-accept-once-and-reload', async () => {
      const previousTeam = await member.locator('#access-team').inputValue()
      expect(previousTeam === team).toBe(false)
      await change(member, 'access.team_invitation.accept', {token})
      await expect(member.locator('#access-team')).toHaveValue(team)
      await reloadTeam(owner, team)
      await assertMember(owner, manifest.memberPrincipalId, 'member', 'active')
      await refresh(owner, 'invitations')
      await expect(row(owner, 'invitations', manifest.memberPrincipalId).locator('[data-status]')).toHaveAttribute('data-status', 'accepted')
      await member.reload()
      await inventory(member, 'teams')
      await expect(member.locator(`#access-team option[value="${team}"]`)).toHaveCount(1)
      await change(member, 'access.team_invitation.accept', {token}, 'failed')
      await expect(member.locator('#access-receipt')).toContainText('Access denied')
      token = undefined
    })
    const expiredTeam = await step('create-short-lived-invitation-team', () => createTeam('expiry'))
    await step('expiry-commits-and-holds-an-uncertain-receipt', async () => {
      await issue(1)
      const invitation = row(owner, 'invitations', manifest.memberPrincipalId)
      const expires = Number((await invitation.textContent()).match(/Expires (\d+)/)?.[1])
      expect(Number.isSafeInteger(expires)).toBe(true)
      await expect.poll(() => Date.now() >= (expires + 1) * 1000, {timeout: 4000}).toBe(true)
      await change(member, 'access.team_invitation.accept', {token}, 'uncertain')
      await expect(member.locator('#access-receipt')).toContainText('Do not submit again')
      await expect(actionButton(member, 'access.team_invitation.accept')).toBeDisabled()
      await refresh(owner, 'invitations')
      await expect(invitation.locator('[data-status]')).toHaveAttribute('data-status', 'expired')
      const committedRevision = await revision(owner, 'invitations')
      await expect(row(owner, 'members', manifest.memberPrincipalId)).toHaveCount(0)
      // Confirm the committed expiry first, then explicitly exercise the separate
      // repeat-denial contract. There is no automatic mutation retry.
      await member.reload()
      await inventory(member, 'teams')
      await change(member, 'access.team_invitation.accept', {token}, 'failed')
      await expect(member.locator('#access-receipt')).toContainText('Access denied')
      await refresh(owner, 'invitations')
      expect(await revision(owner, 'invitations')).toBe(committedRevision)
      receipt.expiry = {team: expiredTeam, committedRevision, repeatZeroVisibleEffects: true}
      token = undefined
    })
    await step('stale-team-epoch-denial-preserves-revoked-membership', async () => {
      const staleTeam = await createTeam('stale-invite')
      await issue(60)
      await owner.locator('[data-access-forget]').click()
      await change(owner, 'access.team.member.add', {principal_id: manifest.memberPrincipalId, role: 'member'})
      await change(owner, 'access.team.member.remove', {principal_id: manifest.memberPrincipalId})
      await refresh(owner, 'invitations')
      await expect(row(owner, 'invitations', manifest.memberPrincipalId)).toContainText('stale_epoch')
      const before = await revision(owner, 'invitations')
      await change(member, 'access.team_invitation.accept', {token}, 'failed')
      await expect(member.locator('#access-receipt')).toContainText('Access denied')
      await refresh(owner, 'members')
      await assertMember(owner, manifest.memberPrincipalId, 'member', 'revoked')
      await refresh(owner, 'invitations')
      expect(await revision(owner, 'invitations')).toBe(before)
      await expect(row(owner, 'invitations', manifest.memberPrincipalId).locator('[data-status]')).toHaveAttribute('data-status', 'pending')
      receipt.staleInvitationTeam = staleTeam
      token = undefined
    })
  })
})
