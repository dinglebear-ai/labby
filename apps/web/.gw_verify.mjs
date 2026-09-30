import { chromium } from 'playwright';

const browser = await chromium.launch({ headless: true });
const page = await browser.newPage({ viewport: { width: 1360, height: 960 } });
const base = 'http://127.0.0.1:3101';

try {
  await page.goto(`${base}/gateway/?id=gw-2`, { waitUntil: 'networkidle' });
  await page.evaluate(() => window.localStorage.clear());
  await page.reload({ waitUntil: 'networkidle' });

  await page.getByRole('heading', { name: 'github-server' }).waitFor();
  await page.getByRole('switch', { name: 'Gateway enabled' }).click();
  await page.getByRole('dialog', { name: 'Disable gateway?' }).waitFor();
  const dialogText = await page.getByRole('dialog').innerText();
  await page.getByRole('button', { name: 'Disable gateway' }).click();
  await page.getByText('Gateway disabled. Catalog change sent to clients.').waitFor();
  await page.getByText('Gateway disabled. This gateway is excluded from the active catalog').waitFor();
  const testDisabled = await page.getByRole('button', { name: 'Test gateway' }).isDisabled();
  const reloadDisabled = await page.getByRole('button', { name: 'Reload gateway' }).isDisabled();
  const enabledSwitchChecked = await page.getByRole('switch', { name: 'Gateway enabled' }).getAttribute('aria-checked');

  await page.goto(`${base}/gateways/`, { waitUntil: 'networkidle' });
  await page.getByRole('link', { name: 'github-server' }).waitFor();
  const enableButtonVisible = await page.getByRole('button', { name: 'Enable gateway' }).first().isVisible();
  const disableButtonCount = await page.getByRole('button', { name: 'Disable gateway' }).count();

  console.log(JSON.stringify({
    ok: true,
    dialogText,
    testDisabled,
    reloadDisabled,
    enabledSwitchChecked,
    enableButtonVisible,
    disableButtonCount,
  }, null, 2));
} catch (error) {
  console.error('VERIFY_ERROR');
  console.error(error);
  process.exitCode = 1;
} finally {
  await browser.close();
}
