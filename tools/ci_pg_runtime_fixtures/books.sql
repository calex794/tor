INSERT INTO books(id,source_identity,target_language,title,visibility,access_level)
VALUES (1,'ci:1','ru','TOR CI public','public','basic'),
       (2,'ci:2','ru','TOR CI hidden','draft','basic'),
       (3,'ci:3','ru','TOR CI held','public','basic');
UPDATE books SET admin_hold=true WHERE id=3;
-- Author/genre fixtures cover public, hidden and held rows with identical metadata.
UPDATE books SET author_name='Жюль', genres=ARRAY['фэнтези','приключения']
WHERE id IN (1,2,3);
