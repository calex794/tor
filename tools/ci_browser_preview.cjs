'use strict';
// Bounded direct Chromium evidence for changed UI when CI artifacts cannot be opened locally.
const assert = require('node:assert/strict');
const {execFileSync} = require('node:child_process');
const {createHash} = require('node:crypto');

module.exports = async function preview(page,name,selector='[data-testid="collection-retired"]') {
  if (process.env.TOR_BROWSER_PROOF_IMAGES !== '1') return;
  assert.equal(process.env.GITHUB_ACTIONS,'true','fixture previews are CI-only');
  assert.match(name,/^[a-z0-9-]{1,90}$/);
  const viewport=page.viewportSize();
  assert.ok(viewport && [390,1440].includes(viewport.width));
  assert.ok([844,900].includes(viewport.height));
  await page.locator(selector).first().scrollIntoViewIfNeeded();
  let bytes,quality;
  for (quality of [55,45,35,25]) {
    bytes=await page.screenshot({type:'jpeg',quality,fullPage:false,animations:'disabled'});
    if (bytes.length<=96*1024) break;
  }
  assert.ok(bytes.length<=96*1024,'fixture preview exceeds 96 KiB');
  const head=execFileSync('git',['rev-parse','HEAD'],{encoding:'utf8'}).trim();
  assert.match(head,/^[0-9a-f]{40}$/);
  const encoded=bytes.toString('base64'),chunks=[];
  for(let offset=0;offset<encoded.length;offset+=8192) chunks.push(encoded.slice(offset,offset+8192));
  console.log('TOR_UI_IMAGE_META '+JSON.stringify({
    name,head,mime:'image/jpeg',...viewport,quality,bytes:bytes.length,
    sha256:createHash('sha256').update(bytes).digest('hex'),chunks:chunks.length,
  }));
  chunks.forEach((chunk,index)=>console.log(`TOR_UI_IMAGE_CHUNK ${name} ${index+1}/${chunks.length} ${chunk}`));
  console.log('TOR_UI_IMAGE_END '+name);
};
