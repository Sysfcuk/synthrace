// @ts-check
const { defineConfig } = require("@playwright/test");

module.exports = defineConfig({
  testDir: ".",
  timeout: 60_000,
  use: {
    baseURL: process.env.BASE_URL || "http://127.0.0.1:8080/",
    viewport: { width: 1280, height: 720 },
    launchOptions: { args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--autoplay-policy=no-user-gesture-required"] },
  },
  projects: [{ name: "chromium", use: { browserName: "chromium" } }],
});
