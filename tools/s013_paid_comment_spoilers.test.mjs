// S013: paired spoiler policy and XSS-safe DOM rendering, without an HTTP server.
import assert from "node:assert/strict";
import test from "node:test";
import {parseCommentSpoilers, maskCommentMentions, appendPaidCommentMarkup}
  from "../crates/web/assets/newboo-paid-comments.mjs";

test("paid S013 pairs are closed by default, empty and unpaired literal", () => {
  assert.deepEqual(parseCommentSpoilers("до ||секрет <b>|| после ||ещё||"), [
    {kind:"text",text:"до "},
    {kind:"spoiler",text:"секрет <b>"},
    {kind:"text",text:" после "},
    {kind:"spoiler",text:"ещё"},
  ]);
  assert.deepEqual(parseCommentSpoilers("||||"), [{kind:"text",text:"||||"}]);
  assert.deepEqual(parseCommentSpoilers("||незакрытый"), [{kind:"text",text:"||незакрытый"}]);
  assert.deepEqual(parseCommentSpoilers(""), [{kind:"text",text:""}]);
  assert.deepEqual(parseCommentSpoilers("😀||秘密||"), [
    {kind:"text",text:"😀"}, {kind:"spoiler",text:"秘密"},
  ]);
});

test("mentions match public Rust S013 masking boundaries", () => {
  assert.equal(maskCommentMentions("@semi_private @[user:42]"), "@пользователь @пользователь");
  assert.equal(maskCommentMentions("a@abc @@abc @ab @[user:0] @[user:42]x"),
    "a@abc @@abc @ab @[user:0] @[user:42]x");
  assert.equal(maskCommentMentions("😀@abc!"), "😀@пользователь!");
  assert.equal(maskCommentMentions("@" + "a".repeat(33)), "@" + "a".repeat(33));
  assert.equal(maskCommentMentions("<script>"), "<script>");
});

test("paid comment DOM never parses user HTML, spoiler content stays closed", () => {
  const original = globalThis.document;
  const createElement = (tag) => ({
    tagName: tag, textContent: "", className: "", children: [],
    append(...nodes) { this.children.push(...nodes); },
  });
  globalThis.document = {
    createElement,
    createTextNode: text => ({tagName:"#text",textContent:text}),
  };
  try {
    const root = createElement("div");
    appendPaidCommentMarkup(root, "Начало ||<script>alert(1)</script> @alice|| конец");
    assert.equal(root.children.length, 3);
    assert.equal(root.children[0].textContent, "Начало ");
    const spoiler = root.children[1];
    assert.equal(spoiler.tagName, "details");
    assert.equal(spoiler.open, undefined);
    assert.equal(spoiler.children[0].tagName, "summary");
    assert.equal(spoiler.children[0].textContent, "Спойлер");
    assert.equal(spoiler.children[1].textContent, "<script>alert(1)</script> @пользователь");
    assert.equal(root.innerHTML, undefined);
    assert.equal(spoiler.innerHTML, undefined);
    assert.equal(root.children[2].textContent, " конец");
  } finally {
    globalThis.document = original;
  }
});
