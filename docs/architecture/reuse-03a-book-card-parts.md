# REUSE-03A — common BookCard presentation primitives

Scope H, with B/C/E/F presentation boundaries retained. Share the common
image markup in public catalogue/home, chapter-end recommendation cards
(organic/editorial) and private library. Share the mini-card cover wrapper
and its image renderer; the library retains its own artwork wrapper.
The saved and purchased library entries share their identical deletion notice.

Do not unify whole cards: public catalogue/home has a confirmed-cover set,
the existing recommendation card always requests the cover route and has
a separate flip interaction, and the library suppresses images of ordinary
or pending deleted books, archived saved editions and unconfirmed covers.
The library's existing None=legacy unknown-cover behavior is retained.
The rendering helper is not an authorization decision. Numeric IDs retain
their decimal representation, including the different signed/unsigned
source types. No policy or image lookup change.

Preserve HTML, CSS classes, lazy loading, decoding, drag behavior, placeholders,
escaped metadata, flow observation attributes, separate reader-facing sources,
private navigation, deletion precedence and all links/forms. No JS, CSS,
queries, routes, migrations, or deployment change. Unit tests assert exact
image/placeholder markup, preserve private suppression cases, and exercise
deletion notice precedence; existing catalogue, library, recommendation and
browser tests remain acceptance gates. Further card consolidation should be
guided by actual shared semantics, not visual similarity alone.
