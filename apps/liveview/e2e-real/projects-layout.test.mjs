import {test} from 'node:test'
import assert from 'node:assert/strict'
import {readFileSync, mkdirSync, writeFileSync, mkdtempSync, rmSync} from 'node:fs'
import {resolve, dirname} from 'node:path'
import {tmpdir} from 'node:os'
import {execFileSync} from 'node:child_process'
import {chromium} from '@playwright/test'

// Offline presentation coverage renders the production component and loads its real styles.
// A private compiled snapshot can be selected while another lane changes unrelated modules.
const root = process.cwd()
const mixRoot = process.env.PROJECTS_LAYOUT_MIX_ROOT || root
const output = process.env.PROJECTS_LAYOUT_OUTPUT || resolve(root, 'test-results/projects-layout')
const css = path => readFileSync(path, 'utf8').replace(/@import "([^"]+)";/g,
  (_, relative) => css(resolve(dirname(path), relative)))

test('Projects heading keeps every action reachable at desktop tablet and mobile widths', async () => {
  const renderDirectory = mkdtempSync(resolve(tmpdir(), 'labby-projects-layout-'))
  const renderPath = resolve(renderDirectory, 'projects.html')
  let html
  try {
    execFileSync('mix', ['run', '--no-start', '-e',
      'File.write!(System.fetch_env!("PROJECTS_LAYOUT_RENDERED_PATH"), Phoenix.LiveViewTest.__render_component__(LabbyConsoleWeb.Endpoint, &LabbyConsoleWeb.ProjectsComponents.page/1, [state: %{LabbyConsole.Projects.new() | teams: [%{id: "layout-team", name: "Layout team", can_create: true}], active_team: "layout-team"}, streams: %{projects: []}, count: 0], []))'],
    {cwd: mixRoot, env: {...process.env, MIX_ENV: 'test', ERL_FLAGS: '+S 4:4', PROJECTS_LAYOUT_RENDERED_PATH: renderPath}, timeout: 180000, encoding: 'utf8'})
    html = readFileSync(renderPath, 'utf8')
  } finally {
    rmSync(renderDirectory, {recursive: true, force: true})
  }
  const styles = [css(resolve(root, 'priv/static/assets/css/app.css')),
    css(resolve(root, 'priv/static/vendor/aurora.css')), css(resolve(root, 'assets/css/workspace.css'))].join('\n')
  const browser = await chromium.launch({headless: true,
    ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH ? {executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH} : {})})
  const context = await browser.newContext()
  const page = await context.newPage()
  // No network or authenticated account is needed by this presentation check.
  await context.route('**/*', route => route.abort())
  const measurements = []
  const failures = []
  mkdirSync(output, {recursive: true})
  try {
    for (const width of [1440, 740, 320]) {
      await page.setViewportSize({width, height: 900})
      await page.setContent(`<div class="labby-overview">${html}</div>`)
      await page.addStyleTag({content: styles})
      for (const longText of [false, true]) {
        if (longText) await page.locator('.lb-page-heading p').evaluate(element => {
          element.textContent = 'Projects for TeamWithAnExceptionallyLongUnbrokenIdentifier and SubjectWithAnExceptionallyLongUnbrokenIdentifier'
        })
        const geometry = await page.evaluate(() => {
          const rect = element => {const r = element.getBoundingClientRect(); return {left:r.left, right:r.right, top:r.top, bottom:r.bottom, width:r.width, height:r.height}}
          return {width:innerWidth, scrollWidth:document.documentElement.scrollWidth,
            heading:rect(document.querySelector('.lb-page-heading')),
            actions:[...document.querySelectorAll('.pj-heading-actions button')].map(element => ({id:element.id, ...rect(element)})),
            headingWrap:getComputedStyle(document.querySelector('.lb-page-heading')).flexWrap,
            actionsWrap:getComputedStyle(document.querySelector('.pj-heading-actions')).flexWrap}
        })
        measurements.push({...geometry, longText})
        await page.screenshot({path:resolve(output, `projects-${width}-${longText ? 'long' : 'normal'}.png`)})
        for (const action of geometry.actions) {
          if (action.left < 0 || action.right > width) failures.push(`${action.id} outside ${width}px viewport, longText=${longText}`)
          assert.ok(action.width >= 36 && action.height >= 36)
          if (width <= 820) assert.ok(action.width >= 44 && action.height >= 44)
        }
        if (geometry.scrollWidth > width) failures.push(`Page width ${geometry.scrollWidth} exceeds ${width}px, longText=${longText}`)
        await page.locator('#projects-lookup').focus()
        await page.keyboard.press('Tab')
        assert.equal(await page.locator(':focus').getAttribute('id'), 'projects-refresh')
        await page.keyboard.press('Tab')
        assert.equal(await page.locator(':focus').getAttribute('id'), 'projects-create')
      }
    }
  } finally {
    writeFileSync(resolve(output, 'geometry.json'), JSON.stringify(measurements, null, 2))
    await context.close()
    await browser.close()
  }
  assert.deepEqual(failures, [])
})
