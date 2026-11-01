// D-07: real desktop/mobile author discount -> reader follow -> paid checkout.
// The browser uses actual Rust handlers, and the test-only PostgreSQL fixture
// supplies fake RUB wallet funding; production has no exposed funding route.
const { chromium } = require('playwright');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const base='http://127.0.0.1:3098';
const db=process.env.APP_URL;
const shots=process.env.TOR_SUBSCRIBER_SHOTS || 'subscriber-shots';
fs.mkdirSync(shots,{recursive:true});
function assert(ok,message){if(!ok)throw new Error(message);}
function pg(sql){
  assert(db && db.startsWith('postgresql://tor_app:') && db.endsWith('/tor_ci'),
    'refuse all non-ci databases');
  return execFileSync('psql',['-X','-A','-t','-q','-v','ON_ERROR_STOP=1','-d',db,'-c',sql],
    {encoding:'utf8',timeout:15000,stdio:['ignore','pipe','pipe']}).trim();
}
function positive(output){
  const rows=String(output).split(/\r?\n/).map(s=>s.trim()).filter(s=>/^[1-9]\d*$/.test(s));
  assert(rows.length===1,'expected one positive DB id: '+output);
  return rows[0];
}
async function register(page,email,display){
  await page.goto(base+'/register');
  await page.getByLabel('Отображаемое имя').fill(display);
  await page.getByLabel('Электронная почта').fill(email);
  await page.getByLabel('Пароль (12–128 символов)').fill('D07ReaderPassword2026');
  await page.getByRole('button',{name:'Создать учётную запись'}).click();
  await page.waitForURL('**/me');
}
async function screen(page,name){
  const sizes=await page.evaluate(()=>({
    content:document.documentElement.scrollWidth,
    viewport:document.documentElement.clientWidth,
    body:document.body.scrollWidth
  }));
  assert(sizes.content<=sizes.viewport+1 && sizes.body<=sizes.viewport+1,
    name+': horizontal overflow '+JSON.stringify(sizes));
  await page.screenshot({path:path.join(shots,'discount-'+name+'.png'),fullPage:true});
}
(async()=>{
  const browser=await chromium.launch({headless:true});
  try{
    for(const [viewport,width,height] of [['desktop',1440,900],['mobile',390,844]]){
      const authorContext=await browser.newContext({viewport:{width,height}});
      const readerContext=await browser.newContext({viewport:{width,height}});
      const guestContext=await browser.newContext({viewport:{width,height}});
      try{
        const author=await authorContext.newPage();
        const reader=await readerContext.newPage();
        const guest=await guestContext.newPage();
        const authorEmail='d07.browser.author.'+viewport+'@example.test';
        const readerEmail='d07.browser.reader.'+viewport+'@example.test';
        await register(author,authorEmail,'Автор D07 '+viewport);
        const authorId=positive(pg("SELECT id FROM users WHERE email_normalized='"+authorEmail+"'"));
        // A real owned Book and published paid V1; no encrypted text is served
        // in this test, only its public price, pinned purchase and ledger.
        const book=positive(pg(
          "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)"+
          " VALUES('ci:d07:browser:"+viewport+"','ru','Проверка скидок','public',"+authorId+") RETURNING id"
        ));
        const chapter=positive(pg(
          "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)"+
          " VALUES("+book+",1,'Платная глава','public','paid',100) RETURNING id"
        ));
        const version=positive(pg(
          "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)"+
          " VALUES("+chapter+",1,'D07-PRIVATE-MARKER') RETURNING id"
        ));
        pg("INSERT INTO chapter_publications(chapter_id,version_id) VALUES("+chapter+","+version+")");
        const authorPage=await author.goto(base+'/me/books/'+book);
        assert(authorPage && authorPage.status()===200,viewport+': author book');
        await author.getByRole('link',{name:'Скидки подписчикам'}).click();
        await author.waitForURL('**/discounts');
        await author.getByLabel('Скидка, % (от 1 до 90)').fill('20');
        await author.getByLabel('Кому доступна').selectOption('90');
        await author.getByLabel('Длительность, дней (от 1 до 90)').fill('30');
        await author.getByRole('button',{name:'Включить скидку'}).click();
        await author.waitForURL('**/discounts');
        assert(await author.getByText('Действующая скидка: 20%',{exact:false}).isVisible(),
          viewport+': campaign did not persist');
        await screen(author,viewport+'-owner-campaign');

        await register(reader,readerEmail,'Читатель D07 '+viewport);
        const readerId=positive(pg("SELECT id FROM users WHERE email_normalized='"+readerEmail+"'"));
        const publicBook=await reader.goto(base+'/books/'+book);
        assert(publicBook && publicBook.status()===200,viewport+': book public');
        await reader.getByRole('button',{name:'Следить за автором'}).click();
        await reader.waitForURL('**/books/'+book);
        assert(await reader.getByRole('button',{name:'Отписаться от автора'}).isVisible(),
          viewport+': creator follow did not start');
        const tooEarly=await reader.goto(base+'/me/books/'+book+'/buy-chapter/'+chapter);
        assert(tooEarly && tooEarly.status()===200,viewport+': buyer quote unavailable');
        assert(await reader.getByText('К списанию: 1,00 ₽').isVisible(),
          viewport+': less than 90 days must not get a discount');
        await screen(reader,viewport+'-not-yet-eligible');
        // Fast-forward ONLY a disposable C/E fact, not the user's system clock.
        pg(
          "UPDATE creator_follow_periods SET started_at=now()-interval '91 days'"+
          " WHERE follower_user_id="+readerId+" AND creator_user_id="+authorId+" AND ended_at IS NULL"
        );
        const paywall=await reader.goto(base+'/reader/'+chapter);
        assert(paywall && paywall.status()===200,viewport+': paid paywall');
        assert(!(await reader.content()).includes('D07-PRIVATE-MARKER'),
          viewport+': private paid body leaked through paywall');
        await reader.getByRole('link',{name:'Купить главу'}).click();
        await reader.waitForURL('**/buy-chapter/'+chapter);
        assert(await reader.getByText('К списанию: 0,80 ₽').isVisible(),
          viewport+': mature subscriber quote did not discount 20%');
        assert(await reader.getByText('со стажем от 90 дней',{exact:false}).isVisible(),
          viewport+': eligibility explanation missing');
        await screen(reader,viewport+'-eligible-quote');

        // Test-only funding source: no public money creation API exists.
        pg("INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)"+
          " VALUES("+readerId+",100,'RUB')");
        pg("INSERT INTO reader_wallet_entries(user_id,kind,delta_minor,balance_after_minor,reference)"+
          " VALUES("+readerId+",'verified_funding',100,100,'ci-d07-browser-"+viewport+"')");
        await reader.getByRole('button',{name:'Купить главу'}).click();
        assert(await reader.getByText('Глава куплена.',{exact:false}).isVisible(),
          viewport+': purchase confirmation');
        assert(await reader.getByText('Списано 0,80 ₽.',{exact:false}).isVisible(),
          viewport+': buyer was charged wrong price');
        const actual=pg(
          "SELECT p.amount_minor||'|'||s.base_price_minor||'|'||s.discount_minor||'|'||"+
          "e.delta_minor||'|'||r.gross_amount_minor "+
          "FROM chapter_purchase_facts p "+
          "JOIN chapter_purchase_discount_snapshots s ON s.purchase_id=p.id "+
          "JOIN reader_wallet_entries e ON e.purchase_id=p.id AND e.kind='chapter_purchase' "+
          "JOIN chapter_purchase_share_snapshots r ON r.purchase_id=p.id "+
          "WHERE p.user_id="+readerId+" AND p.chapter_id="+chapter
        );
        assert(actual==='80|100|20|-80|80',viewport+': financial evidence mismatch '+actual);
        assert(pg("SELECT count(*) FROM chapter_purchase_facts WHERE user_id="+readerId)==='1',
          viewport+': repeated purchase fact');
        await screen(reader,viewport+'-receipt');

        const guestReply=await guest.goto(base+'/me/books/'+book+'/buy-chapter/'+chapter);
        assert(guestReply && new URL(guest.url()).pathname==='/login',
          viewport+': guest sees private offer');
        assert(!(await guest.content()).includes('Обычная цена:'),
          viewport+': guest received a private discount');

        // The author can stop the promotion, but the completed buyer charge
        // cannot be repriced or revoked retroactively.
        await author.getByRole('button',{name:'Остановить скидку'}).click();
        await author.waitForURL('**/discounts');
        assert(await author.getByRole('button',{name:'Включить скидку'}).isVisible(),
          viewport+': author stop failed');
        assert(pg("SELECT paid_minor FROM chapter_purchase_discount_snapshots s "+
          "JOIN chapter_purchase_facts p ON p.id=s.purchase_id "+
          "WHERE p.user_id="+readerId)==='80',viewport+': historical price mutated');
      }finally{
        await authorContext.close();
        await readerContext.close();
        await guestContext.close();
      }
      console.log('D07 '+viewport+' PASS: owner campaign, age gate, paid quote, PostgreSQL settlement, private receipt');
    }
  }finally{
    await browser.close();
  }
})().catch(err=>{console.error(err.stack||err);process.exitCode=1});
