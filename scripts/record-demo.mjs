import { chromium } from 'playwright';
import { execSync } from 'child_process';
import { readdirSync } from 'fs';
import path from 'path';

const URL = 'http://localhost:3000';
const OUT_DIR = path.resolve('static');
const VIDEO_DIR = path.resolve('demo-video');

(async () => {
    const browser = await chromium.launch({ headless: true });
    const context = await browser.newContext({
        viewport: { width: 1280, height: 800 },
        recordVideo: { dir: VIDEO_DIR, size: { width: 1280, height: 800 } },
        colorScheme: 'dark',
    });
    const page = await context.newPage();

    // Load the page and wait for WASM to initialize
    await page.goto(URL);
    await page.waitForTimeout(3000);

    // Upload an instance file directly
    const instancePath = path.resolve('instances/10teams.mps.gz');
    const fileInput = page.locator('#file-input');
    await fileInput.setInputFiles(instancePath);

    // Wait for model to fully load (stats grid becomes visible with content)
    await page.waitForFunction(
        () => document.querySelector('#stats-grid')?.children.length > 0,
        { timeout: 30000 }
    );
    await page.waitForTimeout(2000);

    // Screenshot of the main viewer
    await page.screenshot({ path: path.join(OUT_DIR, 'screenshot.png') });

    // Scroll down slowly to show constraints
    for (let i = 0; i < 3; i++) {
        await page.evaluate(() => window.scrollBy(0, 150));
        await page.waitForTimeout(600);
    }
    await page.waitForTimeout(1000);

    // Scroll back up
    await page.evaluate(() => window.scrollTo({ top: 0, behavior: 'smooth' }));
    await page.waitForTimeout(1500);

    // Click presolve with SCIP
    const presolveBtn = page.locator('#presolve-btn');
    await presolveBtn.click();

    // Wait for presolve to complete (button text changes to "Original")
    await page.waitForFunction(
        () => document.querySelector('#presolve-btn')?.textContent === 'Original',
        { timeout: 30000 }
    );
    await page.waitForTimeout(3000);

    // Scroll to show symmetry and cliques panels
    await page.evaluate(() => {
        const sym = document.querySelector('#symmetry-panel');
        if (sym) sym.scrollIntoView({ behavior: 'smooth', block: 'center' });
    });
    await page.waitForTimeout(2500);

    // Scroll back up
    await page.evaluate(() => window.scrollTo({ top: 0, behavior: 'smooth' }));
    await page.waitForTimeout(1500);

    // Navigate to benchmarks
    await page.goto(URL + '/benchmarks.html');
    await page.waitForTimeout(2500);

    // Toggle through metrics
    await page.locator('.metric-btn[data-metric="nodes"]').click();
    await page.waitForTimeout(1200);

    await page.locator('.metric-btn[data-metric="lp_gap"]').click();
    await page.waitForTimeout(1200);

    await page.locator('.metric-btn[data-metric="time"]').click();
    await page.waitForTimeout(1500);

    await context.close();
    await browser.close();

    // Find the recorded video
    const videos = readdirSync(VIDEO_DIR).filter(f => f.endsWith('.webm'));
    if (videos.length === 0) {
        console.error('No video recorded');
        process.exit(1);
    }
    const videoPath = path.join(VIDEO_DIR, videos[0]);

    // Convert to GIF with ffmpeg — good quality, reasonable size
    const gifPath = path.join(OUT_DIR, 'demo.gif');
    execSync(
        `ffmpeg -y -i "${videoPath}" -vf "fps=10,scale=960:-1:flags=lanczos,split[s0][s1];[s0]palettegen=max_colors=128[p];[s1][p]paletteuse=dither=bayer" -loop 0 "${gifPath}"`,
        { stdio: 'inherit' }
    );

    // Clean up
    execSync(`rm -rf "${VIDEO_DIR}"`);

    console.log(`\nScreenshot: ${path.join(OUT_DIR, 'screenshot.png')}`);
    console.log(`GIF: ${gifPath}`);
})();
