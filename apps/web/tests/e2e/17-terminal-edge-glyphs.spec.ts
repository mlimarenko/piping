import { mkdir, writeFile } from 'node:fs/promises';
import { test, expect } from '@playwright/test';
import { waitForConnectionsPageReady } from './fixtures';

for (const dpr of [1, 2]) {
test.describe(`terminal DPR ${dpr}`, () => {
test.use({ deviceScaleFactor: dpr });
test('real SSH terminal keeps first and final ASCII/Cyrillic cells inside viewer ancestors', async ({ page }) => {
  const proof = process.env.PWA119_TERMINAL_PROOF_DIR;
  const sizes: number[] = [];
  await page.routeWebSocket(/\/terminal-edge\//, socket => {
    socket.onMessage(message => {
      if (typeof message !== 'string' || !message.startsWith('__piping_ws__:')) return;
      const frame = JSON.parse(message.slice('__piping_ws__:'.length));
      if (frame.type !== 'resize') return;
      if(new URL(socket.url()).pathname.endsWith('/terminal-edge')) sizes.push(frame.cols);
      socket.send(`\x1b[2J\x1b[Hroot${'.'.repeat(Math.max(0, frame.cols - 5))}Z\r\nПользователь${'.'.repeat(Math.max(0, frame.cols - 13))}Я`);
    });
  });
  await page.addInitScript(() => {
    (window as Window & { __PIPING_DETACHED_GROUP_HANDOFF__?: unknown }).__PIPING_DETACHED_GROUP_HANDOFF__ = ['terminal-edge','terminal-edge-2'].map(id => ({sessionId:id,hostId:id,hostName:'Terminal edge proof',protocol:'ssh',websocketUrl:`${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}/terminal-edge/${id}`,token:'fixture'}));
  });
  await page.setViewportSize({width:1280,height:900});
  await page.goto('/connections/detached-group?ids=terminal-edge,terminal-edge-2');
  const terminal = page.locator('[data-session-pane-viewer="terminal-edge"]').getByTestId('live-terminal');
  await expect(terminal).toBeVisible();
  await expect.poll(() => sizes.length).toBeGreaterThan(0);
  const initialCols = sizes[0];
  if (initialCols === undefined) throw new Error('The terminal must report its initial column count');
  const receipts = [];
  for (const viewport of [{width:1280,height:900},{width:1280,height:900,split:true},{width:390,height:844}]) {
    if('split' in viewport) {
      await page.getByRole('button',{name:'Pane 1 actions',exact:true}).click();
      await page.getByRole('menuitem',{name:'Split pane 1 side-by-side',exact:true}).click();
      await page.getByRole('combobox',{name:'Session in pane 2',exact:true}).selectOption('terminal-edge-2');
    }
    await page.setViewportSize(viewport);
    if('split' in viewport) await expect.poll(() => sizes.at(-1)).toBeLessThan(initialCols);
    if(viewport.width === 390) await expect.poll(() => sizes.at(-1)).toBeLessThan(initialCols);
    await expect(terminal.locator('.xterm-rows')).toContainText('Пользователь');
    await expect.poll(() => terminal.locator('.xterm-rows > div').nth(1).textContent()).toMatch(/^Пользователь.*Я$/);
    await expect.poll(async () => terminal.evaluate(node => {
      const screen = node.querySelector('.xterm-screen')!.getBoundingClientRect();
      return screen.width <= node.getBoundingClientRect().width + 1;
    })).toBe(true);
    const captureGeometry = () => terminal.evaluate(node => {
      const screen = node.querySelector('.xterm-screen')!;
      const rect = screen.getBoundingClientRect();
      const ancestors = [];
      for (let current: Element | null = screen; current; current = current.parentElement) {
        const style = getComputedStyle(current);
        const box = current.getBoundingClientRect();
        ancestors.push({tag:current.tagName,class:current.className,x:box.x,right:box.right,width:box.width,overflow:style.overflowX,padding:style.padding});
      }
      const rows = [...node.querySelectorAll('.xterm-rows > div')].slice(0,2).map(row => {
        const walker = document.createTreeWalker(row,NodeFilter.SHOW_TEXT);
        const glyphs = [];
        while (walker.nextNode()) {
          const text = walker.currentNode;
          for(let i=0;i<(text.textContent?.length ?? 0);i++) {
            const range = document.createRange(); range.setStart(text,i);range.setEnd(text,i+1);
            const box = range.getBoundingClientRect();
            glyphs.push({text:text.textContent![i],x:box.x,right:box.right});
          }
        }
        const rowStyle = getComputedStyle(row);
        const rowRect = row.getBoundingClientRect();
        const canvas = document.createElement('canvas');
        const context = canvas.getContext('2d')!;
        context.font = rowStyle.font;
        const firstInk = context.measureText(glyphs[0]?.text ?? '');
        const lastInk = context.measureText(glyphs.at(-1)?.text ?? '');
        return {text:row.textContent,first:glyphs[0],last:glyphs.at(-1),ink:{firstLeft:(glyphs[0]?.x ?? 0)-firstInk.actualBoundingBoxLeft,lastRight:(glyphs.at(-1)?.x ?? 0)+lastInk.actualBoundingBoxRight},row:{x:rowRect.x,right:rowRect.right,overflow:rowStyle.overflowX,font:rowStyle.font,letterSpacing:rowStyle.letterSpacing}};
      });
      return {dpr:devicePixelRatio,canvas:node.querySelectorAll('canvas').length,screen:{x:rect.x,right:rect.right,width:rect.width},ancestors,rows};
    });
    let geometry = await captureGeometry();
    // Resizing reflows the old buffer before the asynchronous PTY redraw is
    // parsed. Assert and retain one complete redraw snapshot, not a later race.
    await expect.poll(async () => {
      geometry = await captureGeometry();
      return geometry.rows.map(row => [row.first?.text,row.last?.text]);
    }).toEqual([['r','Z'],['П','Я']]);
    receipts.push({viewport,cols:sizes.at(-1),...geometry});
    if(proof) {
      await mkdir(proof,{recursive:true});
      await writeFile(`${proof}/geometry-dpr${dpr}.json`,JSON.stringify(receipts,null,2));
      for(const theme of ['dark','light']) {
        await page.evaluate(value => {document.documentElement.classList.remove('dark','light');document.documentElement.classList.add(value);},theme);
        await page.screenshot({path:`${proof}/${viewport.width}-${'split' in viewport ? 'split' : 'full'}-dpr${dpr}-${theme}.png`,animations:'disabled'});
      }
    }
    expect(geometry.rows[0]?.first?.text).toBe('r');
    expect(geometry.rows[1]?.first?.text).toBe('П');
    expect(geometry.rows[0]?.last?.text).toBe('Z');
    expect(geometry.rows[1]?.last?.text).toBe('Я');
    for(const ancestor of geometry.ancestors.filter(item => ['hidden','clip','auto','scroll'].includes(item.overflow))) {
      for(const row of geometry.rows) {
        expect.soft(row.first!.x,`${row.first!.text} clipped by ${ancestor.class}`).toBeGreaterThanOrEqual(ancestor.x - 0.5);
        expect.soft(row.last!.right,`${row.last!.text} clipped by ${ancestor.class}`).toBeLessThanOrEqual(ancestor.right + 0.5);
      }
    }
  }
  expect(sizes.at(-1)).toBeLessThan(initialCols);
});
});
}

test('normal right and bottom docks retain first ASCII/Cyrillic glyphs with native SSH transport', async ({ page }) => {
  const proof = process.env.PWA119_TERMINAL_PROOF_DIR;
  const resizes: Array<{ url: string; cols: number }> = [];
  await page.routeWebSocket(/\/terminal-proof\//, socket => {
    socket.onMessage(message => {
      if (typeof message !== 'string' || !message.startsWith('__piping_ws__:')) return;
      const frame = JSON.parse(message.slice('__piping_ws__:'.length));
      if (frame.type !== 'resize') return;
      resizes.push({url:socket.url(),cols:frame.cols});
      socket.send(`\x1b[2J\x1b[Hroot${'.'.repeat(Math.max(0, frame.cols - 5))}Z\r\nПользователь${'.'.repeat(Math.max(0, frame.cols - 13))}Я`);
    });
  });
  await page.setViewportSize({width:1440,height:900});
  await page.goto('/connections');
  await waitForConnectionsPageReady(page);
  await page.evaluate(() => {
    const DemoWebSocket = window.WebSocket;
    // A non-demo relay marker makes the installed shim delegate to its current
    // native constructor, retaining Playwright interception regardless of init order.
    window.WebSocket = class extends DemoWebSocket {
      constructor(url: string | URL, protocols?: string | string[]) {
        super(String(url).replace('wss://demo.invalid/relay/ssh/', `ws://${location.host}/terminal-proof/`), protocols);
      }
    };
  });
  const openSsh = async () => {
    await page.getByTestId(/^quick-access-/).first().click();
    await page.getByRole('menuitem').filter({hasText:/SSH/}).first().click();
  };
  await openSsh();
  const workspace = page.getByTestId('session-workspace');
  await expect(workspace).toBeVisible();
  await expect.poll(() => resizes.length).toBeGreaterThan(0);
  const receipts = [];
  for (const layout of ['right','bottom','right-split','bottom-split']) {
    {
      await workspace.getByRole('button',{name:/session tools|инструменты/i}).first().click();
      const dock = page.getByRole('menuitem',{name:layout.startsWith('right') ? /dock.*right|справа/i : /dock.*bottom|снизу/i});
      if(await dock.count()) await dock.first().click();
      else await page.keyboard.press('Escape');
    }
    if(layout === 'right-split') {
      await openSsh();
      await workspace.getByRole('button',{name:'Pane 1 actions',exact:true}).click();
      await page.getByRole('menuitem',{name:'Split pane 1 side-by-side',exact:true}).click();
      const select = workspace.getByRole('combobox',{name:'Session in pane 2',exact:true});
      const values = await select.locator('option').evaluateAll(options => options.map(option => (option as HTMLOptionElement).value));
      const secondSession = values[1];
      if (secondSession === undefined) throw new Error('Pane 2 must offer a session to display');
      await select.selectOption(secondSession);
    }
    await workspace.evaluate(async node => {
      await Promise.allSettled(node.getAnimations({subtree:true}).map(animation => animation.finished));
    });
    const terminals = workspace.getByTestId('live-terminal').filter({visible:true});
    await expect(terminals.first().locator('.xterm-rows > div').first()).toHaveText(/^root.*Z$/);
    const geometry = [];
    for(const terminal of await terminals.all()) {
      await expect(terminal.locator('.xterm-rows > div').nth(1)).toHaveText(/^Пользователь.*Я$/);
      geometry.push(await terminal.evaluate(node => {
        const screen = node.querySelector('.xterm-screen')!;
        const ancestors = [];
        for(let current: Element | null = screen;current;current=current.parentElement) {
          const rect = current.getBoundingClientRect();
          const style = getComputedStyle(current);
          ancestors.push({class:current.className,x:rect.x,right:rect.right,overflow:style.overflowX,transform:style.transform});
        }
        const first = [...node.querySelectorAll('.xterm-rows > div')].slice(0,2).map(row => {
          const walker = document.createTreeWalker(row,NodeFilter.SHOW_TEXT);
          walker.nextNode();
          const range = document.createRange();range.setStart(walker.currentNode,0);range.setEnd(walker.currentNode,1);
          const rect = range.getBoundingClientRect();
          const hit = document.elementFromPoint((rect.x+rect.right)/2,(rect.top+rect.bottom)/2);
          // Inactive panes intentionally have a transparent focus button above
          // xterm. It intercepts input without painting over the glyph; the
          // opaque pane divider is the obstruction this regression diagnoses.
          const separator = hit?.closest('[role="separator"]');
          const point = {x:(rect.x+rect.right)/2,y:(rect.top+rect.bottom)/2};
          const paintedDivider = separator && [separator,...separator.children].some(element => {
            const style = getComputedStyle(element);
            const box = element.getBoundingClientRect();
            return style.backgroundColor !== 'rgba(0, 0, 0, 0)' && style.backgroundColor !== 'transparent' && point.x >= box.left && point.x <= box.right && point.y >= box.top && point.y <= box.bottom;
          });
          return {text:walker.currentNode.textContent![0],x:rect.x,right:rect.right,unobstructed:!!hit && !paintedDivider,hit:hit?.outerHTML.slice(0,600)};
        });
        return {dpr:devicePixelRatio,canvas:node.querySelectorAll('canvas').length,first,ancestors};
      }));
    }
    receipts.push({layout,resizes:[...resizes],geometry});
    if(proof) {
      await mkdir(proof,{recursive:true});
      await writeFile(`${proof}/normal-docks-geometry.json`,JSON.stringify(receipts,null,2));
      for(const theme of ['dark','light']) {
        await page.evaluate(value => {document.documentElement.classList.remove('dark','light');document.documentElement.classList.add(value);},theme);
        await page.screenshot({path:`${proof}/normal-${layout}-${theme}.png`,animations:'disabled'});
      }
    }
    for(const terminal of geometry) {
      expect(terminal.first.map(glyph => glyph.text)).toEqual(['r','П']);
      for(const glyph of terminal.first) expect.soft(glyph.unobstructed,`${layout}: ${glyph.text} obscured by ${glyph.hit}`).toBe(true);
      for(const ancestor of terminal.ancestors.filter(item => ['hidden','clip','auto','scroll'].includes(item.overflow))) {
        for(const glyph of terminal.first) {
          expect.soft(glyph.x,`${layout}: ${glyph.text} left clipped by ${ancestor.class}`).toBeGreaterThanOrEqual(ancestor.x - 0.5);
          expect.soft(glyph.right,`${layout}: ${glyph.text} right clipped by ${ancestor.class}`).toBeLessThanOrEqual(ancestor.right + 0.5);
        }
      }
    }
  }
});
