"use strict";
(() => {
  const byId = id => document.getElementById(id);
  const board = byId("board");
  const ctx = board.getContext("2d");
  const size = 18, cell = board.width / size;
  const directions = {up: [0,-1], down: [0,1], left: [-1,0], right: [1,0]};
  const keys = {ArrowUp:"up", ArrowDown:"down", ArrowLeft:"left", ArrowRight:"right"};
  const scoreNode = byId("score"), bestNode = byId("best"), status = byId("game-status");
  const start = byId("start"), pause = byId("pause");
  let snake, food, direction, pending, changed, score, timer = null, state = "idle";
  let best = 0;
  try {
    const value = Number(localStorage.getItem("pandaolate.fallback.snake.best"));
    if (Number.isSafeInteger(value) && value >= 0 && value <= size * size - 3) best = value;
  } catch (_) { /* Storage may be disabled. */ }
  bestNode.textContent = best;

  function placeFood() {
    const occupied = new Set(snake.map(([x,y]) => y * size + x));
    const free = [];
    for (let i = 0; i < size * size; i++) if (!occupied.has(i)) free.push(i);
    if (!free.length) return null;
    const n = free[Math.floor(Math.random() * free.length)];
    return [n % size, Math.floor(n / size)];
  }
  function reset() {
    snake = [[8,9],[7,9],[6,9]];
    direction = [1,0]; pending = direction; changed = false; score = 0;
    food = placeFood(); scoreNode.textContent = score;
    draw();
  }
  function draw() {
    if (!ctx) return;
    ctx.fillStyle = "#192820"; ctx.fillRect(0,0,board.width,board.height);
    ctx.strokeStyle = "#23382b"; ctx.lineWidth = 1;
    for (let i = 1; i < size; i++) {
      ctx.beginPath(); ctx.moveTo(i*cell,0); ctx.lineTo(i*cell,board.height); ctx.stroke();
      ctx.beginPath(); ctx.moveTo(0,i*cell); ctx.lineTo(board.width,i*cell); ctx.stroke();
    }
    snake.forEach(([x,y], i) => {
      ctx.fillStyle = i ? "#82b88d" : "#c5ecc0";
      ctx.fillRect(x*cell+2,y*cell+2,cell-4,cell-4);
    });
    if (food) {
      const [x,y] = food;
      ctx.fillStyle = "#e8bc71"; ctx.fillRect(x*cell+3,y*cell+3,cell-6,cell-6);
      ctx.fillStyle = "#fff0bd"; ctx.fillRect(x*cell+6,y*cell+5,2,cell-10);
    }
  }
  function stop(next, message) {
    clearInterval(timer); timer = null; state = next;
    pause.disabled = next !== "paused";
    pause.textContent = next === "paused" ? "Продолжить" : "Пауза";
    start.textContent = next === "idle" ? "Играть" : "Заново";
    status.textContent = message;
  }
  function tick() {
    direction = pending; changed = false;
    const [x,y] = snake[0], head = [x+direction[0], y+direction[1]];
    const [nx,ny] = head;
    const eats = food && nx === food[0] && ny === food[1];
    const body = eats ? snake : snake.slice(0,-1);
    if (nx < 0 || nx >= size || ny < 0 || ny >= size || body.some(([a,b]) => a === nx && b === ny)) {
      stop("over", "Игра окончена. Собрано книг: " + score); return;
    }
    snake.unshift(head);
    if (eats) {
      score++; scoreNode.textContent = score;
      if (score > best) {
        best = score; bestNode.textContent = best;
        try { localStorage.setItem("pandaolate.fallback.snake.best", String(best)); } catch (_) {}
      }
      food = placeFood();
      if (!food) { draw(); stop("over", "Библиотека собрана целиком!"); return; }
    } else snake.pop();
    draw();
  }
  function run() {
    if (!ctx || state === "running") return;
    state = "running"; pause.disabled = false; pause.textContent = "Пауза";
    start.textContent = "Заново"; status.textContent = "Собирайте золотые книги.";
    timer = setInterval(tick, 150);
    board.focus({preventScroll:true});
  }
  function steer(name) {
    const next = directions[name];
    if (state !== "running" || changed || !next) return;
    if (next[0] === -direction[0] && next[1] === -direction[1]) return;
    pending = next; changed = true;
  }
  start.addEventListener("click", () => { stop("idle", ""); reset(); run(); });
  pause.addEventListener("click", () => {
    if (state === "running") stop("paused", "Пауза.");
    else if (state === "paused") run();
  });
  board.addEventListener("keydown", event => {
    if (keys[event.key] && state === "running") {
      event.preventDefault(); steer(keys[event.key]);
    }
  });
  document.querySelectorAll("[data-dir]").forEach(button => {
    button.addEventListener("click", () => steer(button.dataset.dir));
  });
  document.addEventListener("visibilitychange", () => {
    if (document.hidden && state === "running") stop("paused", "Пауза.");
  });
  reset();
  if (!ctx) { start.disabled = true; status.textContent = "Canvas недоступен."; }

  const connection = byId("connection"), check = byId("check");
  let checking = false, lastCheck = 0;
  async function checkSite() {
    if (checking || Date.now() - lastCheck < 5000) return;
    checking = true; lastCheck = Date.now(); check.disabled = true;
    const controller = new AbortController();
    const deadline = setTimeout(() => controller.abort(), 5000);
    try {
      const response = await fetch("/catalog", {method:"GET", headers:{Accept:"text/html"},
        cache:"no-store", credentials:"omit", redirect:"error", referrerPolicy:"no-referrer", signal:controller.signal});
      if (response.body) await response.body.cancel();
      connection.textContent = response.status === 200
        ? "Каталог отвечает. Можно попробовать вернуться на сайт."
        : "Каталог пока недоступен.";
    } catch (_) { connection.textContent = "Связь пока не восстановлена."; }
    finally { clearTimeout(deadline); checking = false; check.disabled = false; }
  }
  check.hidden = false;
  check.addEventListener("click", checkSite);
  // Never automatically navigate, including during a game.
  setInterval(() => { if (!document.hidden) void checkSite(); }, 60000 + Math.floor(Math.random() * 15000));

  async function loadTelegram() {
    try {
      const response = await fetch("/_tor-fallback/config.json", {cache:"no-store", credentials:"omit", referrerPolicy:"no-referrer", redirect:"error"});
      if (!response.ok) return;
      const config = await response.json();
      // Only an explicitly configured public Telegram channel username.
      if (typeof config.telegram_url !== "string" || !/^https:\/\/t\.me\/[A-Za-z][A-Za-z0-9_]{4,31}$/.test(config.telegram_url)) return;
      const link = byId("telegram");
      link.href = config.telegram_url; link.hidden = false;
    } catch (_) { /* A missing/invalid configuration never breaks the page. */ }
  }
  void loadTelegram();
})();
