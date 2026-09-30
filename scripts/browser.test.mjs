import test from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import { mkdtemp, rm, access } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { privateAddress, publicUrl, browse } from './browser.mjs';

test('public URL boundary rejects local addresses and other protocols',async()=>{
  for(const ip of ['127.0.0.1','10.0.0.1','172.16.1.1','192.168.1.1','169.254.169.254','::1','fc00::1','::ffff:127.0.0.1']) assert.equal(privateAddress(ip),true,ip);
  assert.equal(privateAddress('8.8.8.8'),false);
  for(const url of ['file:///etc/passwd','http://127.0.0.1','http://[::1]','https://user:pass@example.com','http://example.com:8888']) await assert.rejects(publicUrl(url));
});

test('Chromium renders and organizes several fresh source pages', {timeout:60000},async t=>{
  const chrome=process.env.YOURSELF_CHROMIUM || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
  try{await access(chrome);}catch{t.skip('Install Chrome/Chromium or set YOURSELF_CHROMIUM');return;}
  const server=http.createServer((req,res)=>{
    res.setHeader('content-type','text/html');
    if(req.url==='/search') res.end('<title>Fixture search</title><a href="/first">First result</a><a href="/second">Second result</a><a href="/first">Duplicate result</a>');
    else if(req.url==='/first') res.end('<title>Old title</title><nav>Remove nav</nav><main>First source evidence<script>document.title="Rendered first"</script></main>');
    else if(req.url==='/second') res.end('<title>Second source</title><article>Second source evidence</article>');
    else {res.statusCode=404;res.end('Missing');}
  });
  await new Promise(r=>server.listen(0,'127.0.0.1',r));
  const profile=await mkdtemp(join(tmpdir(),'agentyou-browser-test-'));
  try{
    const result=await browse({chrome,profile,allow_private:true,query:'fixture',search_url:`http://127.0.0.1:${server.address().port}/search`,max_results:3});
    assert.equal(result.success,true,JSON.stringify(result));
    assert.equal(result.pages.length,2);
    assert.equal(result.pages[0].title,'Rendered first');
    assert.equal(result.pages[0].text,'First source evidence');
    assert.equal(result.pages[1].title,'Second source');
    assert.equal(result.pages[1].text,'Second source evidence');
    assert.ok(result.pages.every(p=>p.url.startsWith('http://127.0.0.1:')));
  }finally{await new Promise(r=>server.close(r));await rm(profile,{recursive:true,force:true});}
});
