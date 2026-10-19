//! Rule templates: the gallery, the preview, adding one, and what each routes.

use axum::http::StatusCode;
use serde_json::json;

use super::{AN_INSTANCE, TestApp};

/// A title as TMDB describes it, with the Arr's tags. No language is a title
/// no source has described.
struct Film {
    id: &'static str,
    tmdb: i64,
    title: &'static str,
    genres: &'static str,
    keywords: &'static str,
    language: Option<&'static str>,
    certification: &'static str,
    tags: &'static str,
}

/// A title of the library Sonarr would hold, with its series type.
const SERIES: &[(&str, &str, &str)] = &[("frieren", "Frieren", "anime")];

const LIBRARY: &[Film] = &[
    Film {
        id: "totoro",
        tmdb: 8392,
        title: "My Neighbor Totoro",
        genres: r#"["Animation","Family","Fantasy"]"#,
        keywords: "[]",
        language: Some("ja"),
        certification: "G",
        tags: "[]",
    },
    Film {
        id: "paddington",
        tmdb: 116149,
        title: "Paddington",
        genres: r#"["Family","Comedy"]"#,
        keywords: "[]",
        language: Some("en"),
        certification: "PG",
        tags: "[]",
    },
    Film {
        id: "spider-verse",
        tmdb: 324857,
        title: "Spider-Man: Into the Spider-Verse",
        genres: r#"["Animation","Action"]"#,
        keywords: "[]",
        language: Some("en"),
        certification: "PG",
        tags: "[]",
    },
    Film {
        id: "akira",
        tmdb: 149,
        title: "Akira",
        genres: r#"["Animation","Science Fiction"]"#,
        keywords: "[]",
        language: Some("ja"),
        certification: "R",
        tags: "[]",
    },
    Film {
        id: "doc",
        tmdb: 1001,
        title: "Free Solo",
        genres: r#"["Documentary"]"#,
        keywords: "[]",
        language: Some("en"),
        certification: "PG-13",
        tags: "[]",
    },
    Film {
        id: "concert",
        tmdb: 1002,
        title: "Stop Making Sense",
        genres: r#"["Documentary","Music"]"#,
        keywords: r#"["concert film"]"#,
        language: Some("en"),
        certification: "PG",
        tags: "[]",
    },
    Film {
        id: "amelie",
        tmdb: 194,
        title: "Amélie",
        genres: r#"["Comedy","Romance"]"#,
        keywords: "[]",
        language: Some("fr"),
        certification: "R",
        tags: "[]",
    },
    Film {
        id: "parasite",
        tmdb: 496243,
        title: "Parasite",
        genres: r#"["Thriller","Drama"]"#,
        keywords: "[]",
        language: Some("ko"),
        certification: "R",
        tags: "[]",
    },
    Film {
        id: "special",
        tmdb: 1004,
        title: "Nanette",
        genres: r#"["Comedy"]"#,
        keywords: r#"["stand-up comedy"]"#,
        language: Some("en"),
        certification: "NR",
        tags: "[]",
    },
    Film {
        id: "goosebumps",
        tmdb: 257445,
        title: "Goosebumps",
        genres: r#"["Family","Horror","Comedy"]"#,
        keywords: "[]",
        language: Some("en"),
        certification: "PG",
        tags: "[]",
    },
    Film {
        id: "family-r",
        tmdb: 1005,
        title: "A Family Rated for Adults",
        genres: r#"["Family","Drama"]"#,
        keywords: "[]",
        language: Some("en"),
        certification: "R",
        tags: "[]",
    },
    Film {
        id: "family-teen",
        tmdb: 1008,
        title: "A Family Rated for Teens",
        genres: r#"["Family","Comedy"]"#,
        keywords: "[]",
        language: Some("en"),
        certification: "TV-14",
        tags: "[]",
    },
    Film {
        id: "family-pg13",
        tmdb: 1009,
        title: "A Family Rated PG-13",
        genres: r#"["Family","Adventure"]"#,
        keywords: "[]",
        language: Some("en"),
        certification: "PG-13",
        tags: "[]",
    },
    Film {
        id: "kon-tiki",
        tmdb: 1006,
        title: "Kon-Tiki",
        genres: r#"["Adventure","Drama"]"#,
        keywords: "[]",
        language: Some("no"),
        certification: "PG-13",
        tags: "[]",
    },
    Film {
        id: "undescribed",
        tmdb: 1007,
        title: "A Film Nobody Described",
        genres: "[]",
        keywords: "[]",
        language: None,
        certification: "",
        tags: "[]",
    },
    Film {
        id: "asked",
        tmdb: 1003,
        title: "Arrival",
        genres: r#"["Drama","Science Fiction"]"#,
        keywords: "[]",
        language: Some("en"),
        certification: "PG-13",
        tags: r#"["3-alice"]"#,
    },
];

/// A Radarr whose films TMDB describes, a `standard` folder mapped, and the
/// interface in `language`.
async fn library(language: &str) -> TestApp {
    let app = TestApp::new().await;
    sqlx::query(AN_INSTANCE).execute(&app.state.pool).await.unwrap();
    sqlx::query(
        "INSERT INTO root_folders (id, instance_id, arr_id, path, accessible, category)
         VALUES ('rf-1', 'inst-1', 1, '/movies/standard', 1, 'standard')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    for (index, film) in LIBRARY.iter().enumerate() {
        sqlx::query(
            "INSERT INTO media (id, instance_id, arr_id, media_type, title, year, tmdb_id,
             current_path, current_root_folder, monitored, has_files, status, tags, added_at)
             VALUES (?, 'inst-1', ?, 'movie', ?, 2000, ?, ?, '/movies/standard', 1, 1,
                     'released', ?, '2026-08-20 10:00:00')",
        )
        .bind(film.id)
        .bind(index as i64 + 10)
        .bind(film.title)
        .bind(film.tmdb)
        .bind(format!("/movies/standard/{}", film.title))
        .bind(film.tags)
        .execute(&app.state.pool)
        .await
        .unwrap();
        let Some(language) = film.language else { continue };
        sqlx::query(
            "INSERT INTO metadata_cache (source, external_id, media_type, genres, keywords,
             original_language, origin_countries, certifications)
             VALUES ('tmdb', ?, 'movie', ?, ?, ?, '[]', ?)",
        )
        .bind(film.tmdb.to_string())
        .bind(film.genres)
        .bind(film.keywords)
        .bind(language)
        .bind(json!({ "US": film.certification }).to_string())
        .execute(&app.state.pool)
        .await
        .unwrap();
    }
    for (index, (id, title, series_type)) in SERIES.iter().enumerate() {
        sqlx::query(
            "INSERT INTO media (id, instance_id, arr_id, media_type, title, year, series_type,
             current_path, current_root_folder, monitored, has_files, status, tags, added_at)
             VALUES (?, 'inst-1', ?, 'series', ?, 2023, ?, ?, '/movies/standard', 1, 1,
                     'continuing', '[]', '2026-08-20 10:00:00')",
        )
        .bind(id)
        .bind(index as i64 + 100)
        .bind(title)
        .bind(series_type)
        .bind(format!("/movies/standard/{title}"))
        .execute(&app.state.pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO arr_quality_profiles (instance_id, arr_id, name)
         VALUES ('inst-1', 1, 'HD-1080p'), ('inst-1', 4, 'Ultra-HD')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    app.list_tmdb().await;
    app.store_setting("ui_language", language).await;
    app
}

fn entry<'a>(gallery: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
    let templates = gallery.as_array().expect("a list of templates");
    templates.iter().find(|template| template["id"] == id).unwrap_or_else(|| panic!("no {id}"))
}

async fn categories_decided(app: &TestApp) -> std::collections::HashMap<String, String> {
    app.simulate().await;
    sqlx::query_as::<_, (String, String)>("SELECT media_id, category FROM media_routing")
        .fetch_all(&app.state.pool)
        .await
        .unwrap()
        .into_iter()
        .collect()
}

async fn add(app: &TestApp, template: &str, body: serde_json::Value) -> serde_json::Value {
    let added = app.post(&format!("/api/v1/rules/templates/{template}/import"), body).await;
    added.assert_status(StatusCode::CREATED).clone()
}

#[tokio::test]
async fn the_gallery_offers_every_template_in_the_readers_language_in_its_order() {
    let app = library("fr").await;

    let gallery = app.get("/api/v1/rules/templates").await;
    let gallery = gallery.assert_ok().clone();

    let ids: Vec<&str> = gallery
        .as_array()
        .unwrap()
        .iter()
        .map(|template| template["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        ["4k", "concerts", "documentaries", "children", "anime", "foreign-films", "requesters"]
    );
    let children = entry(&gallery, "children");
    assert_eq!(children["name"], "Enfants et famille");
    assert_eq!(children["category"], "enfants");
    assert_eq!(children["added_version"], serde_json::Value::Null);
    assert_eq!(children["rule_ids"], json!([]));
    assert_eq!(entry(&gallery, "foreign-films")["parameter"], "languages");
    assert_eq!(entry(&gallery, "foreign-films")["languages"], json!(["fr", "en"]));
    assert_eq!(
        entry(&gallery, "requesters")["category"],
        serde_json::Value::Null,
        "each requester has their own"
    );
}

/// A French library already sorting children into `kids` is offered `kids`,
/// not a new `enfants` with no folder.
#[tokio::test]
async fn a_template_suggests_the_category_the_library_already_has() {
    let app = library("fr").await;
    sqlx::query("INSERT INTO categories (id, name) VALUES ('cat-kids', 'kids')")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let gallery = app.get("/api/v1/rules/templates").await;
    let gallery = gallery.assert_ok();
    let preview = app.post("/api/v1/rules/templates/children/preview", json!({})).await;

    assert_eq!(entry(gallery, "children")["category"], "kids");
    assert_eq!(
        entry(gallery, "documentaries")["category"],
        "documentaires",
        "no documentaries category yet"
    );
    assert_eq!(preview.assert_ok()["creates"], json!([]));
}

/// Added again, a template offers what it was added with: offering the
/// suggestions would orphan the category the user mapped, and drop every
/// requester left unticked.
#[tokio::test]
async fn the_gallery_offers_a_template_with_the_answers_it_was_added_with() {
    let app = library("en").await;
    add(&app, "documentaries", json!({ "category": "docs" })).await;
    add(&app, "foreign-films", json!({ "languages": ["en", "fr", "ko"] })).await;
    let alice = json!({ "requesters": [{ "tag": "3-alice", "category": "alice-asks" }] });
    let added = add(&app, "requesters", alice).await;

    let gallery = app.get("/api/v1/rules/templates").await;
    let gallery = gallery.assert_ok();

    assert_eq!(entry(gallery, "documentaries")["category"], "docs");
    assert_eq!(entry(gallery, "documentaries")["added_version"], 1);
    assert_eq!(entry(gallery, "documentaries")["rule_ids"].as_array().unwrap().len(), 1);
    assert_eq!(entry(gallery, "foreign-films")["languages"], json!(["en", "fr", "ko"]));
    assert_eq!(entry(gallery, "requesters")["requesters"][0]["category"], "alice-asks");
    assert_eq!(entry(gallery, "requesters")["requesters"][0]["added"], true);
    assert_eq!(entry(gallery, "requesters")["rule_ids"][0], added["rules"][0]["id"]);
}

#[tokio::test]
async fn a_template_reading_keywords_names_the_sources_that_answer_them_until_one_is_on() {
    let app = TestApp::new().await;

    let gallery = app.get("/api/v1/rules/templates").await;
    let gallery = gallery.assert_ok().clone();
    // AniList and Tenrai give keywords too, about anime alone.
    assert_eq!(entry(&gallery, "concerts")["needs"], json!([{ "id": "tmdb", "name": "TMDB" }]));
    assert_eq!(
        entry(&gallery, "documentaries")["needs"],
        json!([]),
        "a genre is answered by the Arr"
    );

    // Listed without a key, TMDB still answers nothing.
    app.list_tmdb().await;
    let gallery = app.get("/api/v1/rules/templates").await;
    assert_ne!(entry(gallery.assert_ok(), "concerts")["needs"], json!([]));

    app.store_setting("tmdb_api_key", &app.state.secrets.seal("tmdb-key-not-a-secret").unwrap())
        .await;
    let gallery = app.get("/api/v1/rules/templates").await;
    assert_eq!(entry(gallery.assert_ok(), "concerts")["needs"], json!([]));
}

#[tokio::test]
async fn every_template_renders_rules_the_validator_accepts_in_every_language() {
    let app = library("en").await;
    for language in crate::localization::languages() {
        app.store_setting("ui_language", language.code).await;
        for template in crate::services::templates::CATALOGUE {
            let body = match template.id {
                "requesters" => {
                    json!({ "requesters": [{ "tag": "3-alice", "category": "alice" }] })
                }
                "4k" => json!({ "quality_profiles": ["Ultra-HD"] }),
                _ => json!({}),
            };
            let preview =
                app.post(&format!("/api/v1/rules/templates/{}/preview", template.id), body).await;
            let preview = preview.assert_ok();
            for planned in preview["rules"].as_array().unwrap() {
                let errors: Vec<&serde_json::Value> = planned["issues"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|issue| issue["severity"] == "error")
                    .collect();
                assert!(errors.is_empty(), "{} in {}: {errors:?}", template.id, language.code);
            }
        }
    }
}

#[tokio::test]
async fn each_template_sends_the_titles_it_claims_an_anime_for_all_ages_to_the_children() {
    let app = library("en").await;
    for template in
        ["requesters", "foreign-films", "anime", "children", "documentaries", "concerts"]
    {
        let body = match template {
            "requesters" => json!({ "requesters": [{ "tag": "3-alice", "category": "alice" }] }),
            "foreign-films" => json!({ "languages": ["en", "fr"] }),
            _ => json!({}),
        };
        add(&app, template, body).await;
    }

    let decided = categories_decided(&app).await;

    assert_eq!(decided["totoro"], "kids");
    assert_eq!(decided["paddington"], "kids", "a family film without animation");
    assert_eq!(decided["spider-verse"], "standard", "animation alone is not for children");
    assert_eq!(decided["goosebumps"], "standard", "a family film with horror");
    assert_eq!(decided["family-r"], "standard", "a family film rated for adults");
    assert_eq!(decided["family-teen"], "standard", "a family film rated for teenagers");
    assert_eq!(decided["family-pg13"], "kids", "PG-13 asks a parent's guidance only");
    assert_eq!(decided["akira"], "anime");
    assert_eq!(decided["frieren"], "anime", "a series Sonarr types as anime");
    assert_eq!(decided["doc"], "documentaries");
    assert_eq!(decided["concert"], "concerts", "concerts go before documentaries");
    assert_eq!(decided["special"], "concerts", "a stand-up special");
    assert_eq!(decided["amelie"], "standard", "French is a language the household watches in");
    assert_eq!(decided["parasite"], "foreign-films");
    assert_eq!(decided["undescribed"], "standard", "a film of unknown language is not foreign");
    assert_eq!(decided["asked"], "alice");
}

/// The interface's language and English, as TMDB writes them: Norwegian is
/// `no`, whatever the interface calls it.
#[tokio::test]
async fn the_languages_a_household_watches_in_default_to_the_interface_and_english() {
    for (interface, own) in [("fr", "amelie"), ("nb_NO", "kon-tiki")] {
        let app = library(interface).await;
        add(&app, "foreign-films", json!({})).await;

        let decided = categories_decided(&app).await;

        assert_eq!(decided[own], "standard", "{interface}");
        assert_ne!(decided["parasite"], "standard", "{interface}");
        assert_eq!(decided["paddington"], "standard", "{interface}: English");
    }
}

/// The engine compares ISO 639-1 codes. One stored as typed, `fra`, would
/// match no film, and every French film would count as foreign.
#[tokio::test]
async fn a_language_is_stored_as_its_code_and_one_naming_none_is_refused() {
    let app = library("en").await;

    let unknown = json!({ "languages": ["en", "klingon"] });
    let refused = app.post("/api/v1/rules/templates/foreign-films/import", unknown).await;
    let refused = refused.assert_status(StatusCode::BAD_REQUEST);
    assert!(refused.to_string().contains("klingon"), "{refused}");

    let typed = json!({ "languages": ["FRA", "English", "fr-CA"] });
    let added = add(&app, "foreign-films", typed).await;
    let conditions = &added["rules"][0]["conditions"];
    assert_eq!(conditions[1], json!({ "type": "original_language_not", "value": ["fr", "en"] }));
}

#[tokio::test]
async fn a_template_refuses_what_it_does_not_read_and_a_name_no_category_may_hold() {
    let app = library("en").await;
    let refused = |template: &'static str, body: serde_json::Value| {
        let app = &app;
        async move {
            let path = format!("/api/v1/rules/templates/{template}/import");
            app.post(&path, body).await.assert_status(StatusCode::BAD_REQUEST).clone()
        }
    };

    refused("children", json!({ "languages": ["en"] })).await;
    refused("children", json!({ "requesters": [] })).await;
    refused("requesters", json!({ "category": "x", "requesters": [] })).await;
    refused("requesters", json!({ "requesters": [{ "tag": "anime", "category": "x" }] })).await;
    refused("children", json!({ "category": "my kids" })).await;
    let missing = app.post("/api/v1/rules/templates/westerns/preview", json!({})).await;
    missing.assert_status(StatusCode::NOT_FOUND);
    assert_eq!(app.count("SELECT COUNT(*) FROM rules").await, 0);
}

#[tokio::test]
async fn two_templates_may_share_a_category_which_is_created_once() {
    let app = library("en").await;

    let first = add(&app, "documentaries", json!({ "category": "docs" })).await;
    let second = add(&app, "concerts", json!({ "category": "docs" })).await;
    let existing = add(&app, "anime", json!({ "category": "standard" })).await;

    assert_eq!(first["created"], json!(["docs"]));
    assert_eq!(second["created"], json!([]));
    assert_eq!(existing["created"], json!([]));
    assert_eq!(app.count("SELECT COUNT(*) FROM categories WHERE name = 'docs'").await, 1);
}

#[tokio::test]
async fn templates_go_after_the_rules_written_by_hand_in_the_catalogue_order() {
    let app = library("en").await;
    let mine = app
        .post(
            "/api/v1/rules",
            json!({
                "name": "Mine",
                "media_type": "both",
                "conditions": [{ "type": "genre_contains", "value": ["Thriller"] }],
                "target_category": "standard"
            }),
        )
        .await;
    let mine = mine.assert_status(StatusCode::CREATED)["priority"].as_i64().unwrap();

    let anime = add(&app, "anime", json!({})).await;
    let children = add(&app, "children", json!({})).await;

    let priority = |rule: &str| {
        let rules = app.state.pool.clone();
        let rule = rule.to_string();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT priority FROM rules WHERE id = ?")
                .bind(rule)
                .fetch_one(&rules)
                .await
                .unwrap()
        }
    };
    let anime_first = priority(anime["rules"][0]["id"].as_str().unwrap()).await;
    let children_rule = priority(children["rules"][0]["id"].as_str().unwrap()).await;
    assert!(mine < children_rule, "the hand-written rule stays first");
    assert!(children_rule < anime_first, "children rank before anime");
    assert_eq!(children["rules"][0]["template"], "children@1");
}

#[tokio::test]
async fn a_template_added_twice_is_refused_unless_it_replaces_the_first() {
    let app = library("en").await;
    add(&app, "anime", json!({})).await;

    let again = app.post("/api/v1/rules/templates/anime/import", json!({})).await;
    assert_eq!(again.assert_status(StatusCode::CONFLICT)["reason"], "template_added");
    let previewed = app.post("/api/v1/rules/templates/anime/preview", json!({})).await;
    assert_eq!(previewed.assert_status(StatusCode::CONFLICT)["reason"], "template_added");

    let replaced = add(&app, "anime", json!({ "replace": true })).await;
    assert_eq!(replaced["replaced"], 2);
    assert_eq!(app.count("SELECT COUNT(*) FROM rules WHERE template = 'anime@1'").await, 2);
}

#[tokio::test]
async fn the_preview_names_the_rule_that_would_keep_titles_from_the_template() {
    let app = library("en").await;
    app.post(
        "/api/v1/rules",
        json!({
            "name": "Every animation",
            "media_type": "both",
            "conditions": [{ "type": "genre_contains", "value": ["Animation"] }],
            "target_category": "standard"
        }),
    )
    .await
    .assert_status(StatusCode::CREATED);

    let preview = app.post("/api/v1/rules/templates/children/preview", json!({})).await;
    let preview = preview.assert_ok();

    assert_eq!(preview["taken_by"][0]["rule_name"], "Every animation");
    assert_eq!(preview["taken_by"][0]["titles"], 1, "Totoro");
    assert_eq!(preview["creates"], json!(["kids"]));
    assert_eq!(app.count("SELECT COUNT(*) FROM rules").await, 1, "a preview stores nothing");
}

/// What the preview counts is what adding it routes, and a category it
/// creates has no folder, said once by the preview rather than by each rule.
#[tokio::test]
async fn the_preview_counts_the_titles_adding_the_template_routes() {
    let app = library("en").await;

    let preview = app.post("/api/v1/rules/templates/children/preview", json!({})).await;
    let preview = preview.assert_ok().clone();
    let added = add(&app, "children", json!({})).await;
    let decided = categories_decided(&app).await;

    assert_eq!(preview["rules"][0]["rule"]["priority"], added["rules"][0]["priority"]);
    let kids = decided.values().filter(|category| *category == "kids").count();
    assert_eq!(preview["reach"]["titles"], kids);
    assert_eq!(preview["reach"]["unmapped"], kids, "kids has no folder yet");
    assert_eq!(preview["reach"]["to_move"], 0);
    let titles: Vec<&str> = preview["titles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|title| title["media_title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["A Family Rated PG-13", "My Neighbor Totoro", "Paddington"]);
    assert_eq!(preview["titles"][0]["action"], "skip");
    assert_eq!(preview["titles"][0]["rule_name"], "Children and family");
    let issues = &preview["rules"][0]["issues"];
    assert!(!issues.to_string().contains("ValidationCategoryUnmapped"), "{issues}");
}

#[tokio::test]
async fn a_title_already_in_its_folder_counts_as_in_place() {
    let app = library("en").await;
    let preview =
        app.post("/api/v1/rules/templates/anime/preview", json!({ "category": "standard" })).await;
    let reach = &preview.assert_ok()["reach"];
    assert_eq!(reach["titles"], 3, "Totoro, Akira and Frieren");
    assert_eq!(reach["in_place"], 3);
}

#[tokio::test]
async fn the_requester_template_offers_the_seerr_tags_and_wants_one_chosen() {
    let app = library("en").await;

    let gallery = app.get("/api/v1/rules/templates").await;
    let requesters = &entry(gallery.assert_ok(), "requesters")["requesters"];
    let alice = json!({ "tag": "3-alice", "name": "alice", "titles": 1, "category": "alice", "added": false });
    assert_eq!(requesters, &json!([alice]));

    let none = app.post("/api/v1/rules/templates/requesters/import", json!({})).await;
    none.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(app.count("SELECT COUNT(*) FROM rules").await, 0);
}

/// An older Seerr writes `3 - alice`, and a tag keeps the name a person had
/// when they asked: one person, whichever tag their titles carry.
#[tokio::test]
async fn a_requester_tagged_in_several_forms_is_one_person_routed_by_each() {
    let app = library("en").await;
    for (id, tag) in [("amelie", "3 - alice"), ("paddington", "3-alicia")] {
        sqlx::query("UPDATE media SET tags = json_array(?) WHERE id = ?")
            .bind(tag)
            .bind(id)
            .execute(&app.state.pool)
            .await
            .unwrap();
    }

    let gallery = app.get("/api/v1/rules/templates").await;
    assert_eq!(entry(gallery.assert_ok(), "requesters")["requesters"].as_array().unwrap().len(), 1);
    let alice = json!({ "requesters": [{ "tag": "3-alice", "category": "alice" }] });
    add(&app, "requesters", alice).await;

    let decided = categories_decided(&app).await;
    assert_eq!(decided["asked"], "alice");
    assert_eq!(decided["amelie"], "alice");
    assert_eq!(decided["paddington"], "alice", "renamed since");
}

/// An export restored, a template's rules still come from it: otherwise the
/// gallery offers to add it again and its rules count as written by hand.
#[tokio::test]
async fn a_rule_bundle_keeps_the_template_each_rule_came_from() {
    let app = library("en").await;
    add(&app, "anime", json!({})).await;

    let rules = app.get("/api/v1/rules/export").await.assert_ok().clone();
    assert_eq!(rules["rules"][0]["template"], "anime@1");
    let replace = json!({ "bundle": rules, "replace": true });
    app.post("/api/v1/rules/import", replace).await.assert_ok();
    assert_eq!(app.count("SELECT COUNT(*) FROM rules WHERE template = 'anime@1'").await, 2);

    let config = app.get("/api/v1/config/export").await.assert_ok().clone();
    let replace = json!({ "bundle": config, "replace_rules": true });
    app.post("/api/v1/config/import", replace).await.assert_ok();
    assert_eq!(app.count("SELECT COUNT(*) FROM rules WHERE template = 'anime@1'").await, 2);

    let mut forged = rules.clone();
    forged["rules"][0]["template"] = json!("anime; DROP");
    app.post("/api/v1/rules/import", json!({ "bundle": forged, "replace": true }))
        .await
        .assert_ok();
    assert_eq!(app.count("SELECT COUNT(*) FROM rules WHERE template IS NULL").await, 1);
}

/// Refused halfway, an addition leaves the rules as they were, the earlier
/// copy it was replacing included.
#[tokio::test]
async fn an_addition_past_the_rule_limit_changes_nothing() {
    let app = library("en").await;
    for (id, tag) in [("amelie", "4-bob"), ("paddington", "5-carol")] {
        sqlx::query("UPDATE media SET tags = json_array(?) WHERE id = ?")
            .bind(tag)
            .bind(id)
            .execute(&app.state.pool)
            .await
            .unwrap();
    }
    let alice = json!({ "tag": "3-alice", "category": "alice" });
    let bob = json!({ "tag": "4-bob", "category": "bob" });
    let carol = json!({ "tag": "5-carol", "category": "carol" });
    add(&app, "requesters", json!({ "requesters": [alice] })).await;
    for index in 1..crate::api::rules::MAX_RULES {
        sqlx::query(
            "INSERT INTO rules (id, name, priority, enabled, media_type, conditions, exclusions,
             match_mode, target_category) VALUES (?, ?, ?, 1, 'both', '[]', '[]', 'all', 'standard')",
        )
        .bind(format!("filler-{index}"))
        .bind(format!("Filler {index}"))
        .bind(index * 10 + 1000)
        .execute(&app.state.pool)
        .await
        .unwrap();
    }

    // Alice's rule out, two in: the second is one too many.
    let others = json!({ "requesters": [bob, carol], "replace": true });
    let refused = app.post("/api/v1/rules/templates/requesters/import", others).await;
    refused.assert_status(StatusCode::BAD_REQUEST);

    let kept: Vec<String> =
        sqlx::query_scalar("SELECT target_category FROM rules WHERE template = 'requesters@1'")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(kept, ["alice"]);
    assert_eq!(app.count("SELECT COUNT(*) FROM categories WHERE name = 'bob'").await, 0);
}

/// A rule the user edited survives a requester added beside it, and a
/// requester left unticked loses theirs.
#[tokio::test]
async fn adding_a_requester_leaves_the_others_rules_as_they_stand() {
    let app = library("en").await;
    sqlx::query("UPDATE media SET tags = '[\"4-bob\"]' WHERE id = 'amelie'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    let alice = json!({ "tag": "3-alice", "category": "alice" });
    let bob = json!({ "tag": "4-bob", "category": "bob" });
    let first = add(&app, "requesters", json!({ "requesters": [alice] })).await;
    let alice_rule = first["rules"][0]["id"].as_str().unwrap().to_string();
    sqlx::query("UPDATE rules SET name = 'Alice, edited' WHERE id = ?")
        .bind(&alice_rule)
        .execute(&app.state.pool)
        .await
        .unwrap();

    let both = json!({ "requesters": [alice, bob], "replace": true });
    let preview = app.post("/api/v1/rules/templates/requesters/preview", both.clone()).await;
    let preview = preview.assert_ok().clone();
    let updated = add(&app, "requesters", both).await;

    assert_eq!(preview["rules"].as_array().unwrap().len(), 1, "bob's rule alone is written");
    assert_eq!(preview["reach"]["titles"], 2, "alice's title and bob's");
    assert_eq!(updated["replaced"], 0);
    assert_eq!(updated["rules"][0]["name"], "Requested by bob");
    let name: String = sqlx::query_scalar("SELECT name FROM rules WHERE id = ?")
        .bind(&alice_rule)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(name, "Alice, edited");

    let bob_only = json!({ "requesters": [bob], "replace": true });
    let removed = add(&app, "requesters", bob_only).await;
    assert_eq!(removed["replaced"], 1);
    assert_eq!(removed["rules"], json!([]));
    assert_eq!(app.count("SELECT COUNT(*) FROM rules WHERE template = 'requesters@1'").await, 1);
}

#[test]
fn a_requester_category_is_a_name_a_category_may_hold() {
    let english = crate::localization::Localizer::new("en");
    let accepted = |id: &str, name: &str| {
        let category = crate::services::templates::requester_category(id, name);
        crate::api::categories::normalise(&category, &english).expect(name);
        category
    };
    assert_eq!(accepted("1", "Bob Smith"), "bob-smith");
    assert_eq!(accepted("2", "le\u{301}a"), "léa");
    assert_eq!(accepted("3", "a  b"), "a-b");
    assert_eq!(accepted("4", "🎬✨"), "seerr-4");
    assert_eq!(accepted("5", &"x".repeat(80)).chars().count(), 64);
    assert_eq!(accepted("6", &format!("{}-b", "a".repeat(63))), "a".repeat(63));
}

/// The 4K template offers the profiles whose name reads as 4K, sends each title of one to its
/// category whatever its genre, and refuses a profile no instance defines.
#[tokio::test]
async fn the_4k_template_routes_the_titles_of_the_profiles_chosen() {
    let app = library("en").await;
    app.execute(&["UPDATE media SET quality_profile = 'Ultra-HD' WHERE id = 'amelie'"]).await;

    let gallery = app.get("/api/v1/rules/templates").await;
    let uhd = entry(gallery.assert_ok(), "4k");
    assert_eq!(
        (&uhd["parameter"], &uhd["quality_profiles"]),
        (&json!("quality_profiles"), &json!(["Ultra-HD"]))
    );

    let preview = app.post("/api/v1/rules/templates/4k/preview", json!({})).await;
    let preview = preview.assert_ok();
    assert_eq!(preview["reach"]["titles"], 1, "{preview}");
    assert_eq!(preview["titles"][0]["media_id"], "amelie");

    let unknown = json!({ "quality_profiles": ["Dolby Vision"] });
    let refused = app.post("/api/v1/rules/templates/4k/import", unknown).await;
    let message = refused.assert_status(StatusCode::BAD_REQUEST)["message"].as_str().unwrap();
    assert!(message.contains("Dolby Vision"), "{message}");
    let none =
        app.post("/api/v1/rules/templates/4k/import", json!({ "quality_profiles": [] })).await;
    none.assert_status(StatusCode::BAD_REQUEST);
    let elsewhere = json!({ "quality_profiles": ["Ultra-HD"] });
    let unread = app.post("/api/v1/rules/templates/anime/import", elsewhere).await;
    unread.assert_status(StatusCode::BAD_REQUEST);

    let added = add(&app, "4k", json!({ "quality_profiles": ["ultra-hd"] })).await;
    let rule = &added["rules"][0];
    assert_eq!(
        rule["conditions"],
        json!([{ "type": "quality_profile_in", "value": ["Ultra-HD"] }])
    );
    assert_eq!(rule["target_category"], "4k");
    let gallery = app.get("/api/v1/rules/templates").await;
    assert_eq!(entry(gallery.assert_ok(), "4k")["added_version"], 1);
}
