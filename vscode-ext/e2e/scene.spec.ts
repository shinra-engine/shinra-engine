import { test, expect } from '@playwright/test';

const BASE = 'http://localhost:5812';

// ---- /scene HTTP API --------------------------------------------------------

test('GET /scene returns an object with name and nodes', async ({ request }) => {
    const resp = await request.get(`${BASE}/scene`);
    expect(resp.status()).toBe(200);
    const body = await resp.json();
    expect(typeof body.name).toBe('string');
    expect(Array.isArray(body.nodes)).toBe(true);
});

test('POST /scene replaces the scene and GET reflects it', async ({ request }) => {
    const scene = {
        name: 'e2e-scene',
        nodes: [
            {
                name: 'cube',
                transform: {
                    translation: [1.0, 2.0, 3.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0],
                },
            },
        ],
    };

    const postResp = await request.post(`${BASE}/scene`, { data: scene });
    expect(postResp.status()).toBe(204);

    const getResp = await request.get(`${BASE}/scene`);
    expect(getResp.status()).toBe(200);
    const body = await getResp.json();
    expect(body.name).toBe('e2e-scene');
    expect(body.nodes).toHaveLength(1);
    expect(body.nodes[0].name).toBe('cube');
    expect(body.nodes[0].transform.translation).toEqual([1.0, 2.0, 3.0]);
});

test('POST /scene/save + POST /scene/load round-trips via filesystem', async ({
    request,
}) => {
    const scenePath = '/tmp/shinra-e2e.scn.ron';

    // Establish known scene state
    await request.post(`${BASE}/scene`, {
        data: { name: 'saved', nodes: [] },
    });

    const saveResp = await request.post(`${BASE}/scene/save`, {
        data: { path: scenePath },
    });
    expect(saveResp.status()).toBe(204);

    // Overwrite in-memory scene so load is meaningful
    await request.post(`${BASE}/scene`, {
        data: { name: 'overwritten', nodes: [] },
    });

    const loadResp = await request.post(`${BASE}/scene/load`, {
        data: { path: scenePath },
    });
    expect(loadResp.status()).toBe(204);

    const getResp = await request.get(`${BASE}/scene`);
    const body = await getResp.json();
    expect(body.name).toBe('saved');
});

test('POST /scene/load with missing file returns 404', async ({ request }) => {
    const resp = await request.post(`${BASE}/scene/load`, {
        data: { path: '/tmp/does-not-exist-shinra-e2e.scn.ron' },
    });
    expect(resp.status()).toBe(404);
});

// ---- webview page -----------------------------------------------------------

test('GET / serves the H.264 viewport HTML page', async ({ page }) => {
    await page.goto(`${BASE}/`);
    const canvas = page.locator('canvas#c');
    await expect(canvas).toBeVisible();
    await expect(canvas).toHaveAttribute('width', '512');
    await expect(canvas).toHaveAttribute('height', '384');
});
