// Smoke test for a deployed (or locally served) build. Forces the WebGL2 path,
// because headless CI has no WebGPU adapter; WebGPU is verified manually per milestone.
const { test, expect } = require("@playwright/test");
const { PNG } = require("pngjs");

const IGNORED = [/integrity` attribute/, /powerPreference option/];

test("boots on WebGL2, renders a non-black frame, starts audio, no console errors", async ({ page }) => {
  const errors = [];
  let bootLine = "";
  page.on("console", (m) => {
    const t = m.text();
    if (t.startsWith("SYNTHRACE M0:")) bootLine = t;
    if ((m.type() === "error" || m.type() === "warning") && !IGNORED.some((r) => r.test(t))) errors.push(t);
  });
  page.on("pageerror", (e) => errors.push(String(e)));

  await page.goto("?backend=gl");
  await expect.poll(() => bootLine, { timeout: 30_000 }).toContain("backend=Gl");
  expect(bootLine).toContain("hdr=Rgba16Float");

  await page.click("#start");
  await page.waitForTimeout(2500);
  await expect(page.locator("#hud")).toContainText("audio running");

  const shot = PNG.sync.read(await page.screenshot({ path: "test-results/m0-webgl2.png" }));
  let lit = 0;
  for (let i = 0; i < shot.data.length; i += 4 * 97) {
    if (shot.data[i] + shot.data[i + 1] + shot.data[i + 2] > 60) lit++;
  }
  const samples = shot.data.length / (4 * 97);
  expect(lit / samples).toBeGreaterThan(0.3);
  expect(errors).toEqual([]);
});
