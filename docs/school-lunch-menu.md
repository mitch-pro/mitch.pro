# School lunch menu

The school dashboard shows the district's published high school lunch choices.
It uses the official source linked from [RJUHSD Food Services](https://www.rjuhsd.us/departments/food-services)
and [Woodcreek Nutrition](https://woodcreek.rjuhsd.us/services/health-office/nutrition):

- [District menus](https://www.rjuhsdfoodservice.com/index.php?sid=0212112146052111&page=menus)
- The district's linked ISITE Web Menus viewer and its public GraphQL service at
  `https://api.schoolnutritionandfitness.com/graphql`.

During verification on October 25, 2026 (Pacific time), the official Lunch link
opened August 2026. The date-specific public feed had no entries for late October
or early November. The page therefore labels those food choices with their
published month and explicitly says when no menu has been posted for the selected
date. It never repeats an old month's menu as today's food.

## Data flow

`GET /api/school-menu?school=woodcreek&date=YYYY-MM-DD` is public. It validates the
school against the existing six-campus list and accepts the client's Pacific date
only when it is within one day of the server's UTC date. No arbitrary upstream
URLs are accepted.

`routes/school_menu.rs` finds the Lunch viewer ID in the official menus page,
then reads that published menu's dates and food names. It also requests exact
dates for the next 45 days from the same menu type, covering the first school day
after district breaks. The document ID is discovered each refresh; it is not
pinned to August. Non-food headings, hidden/deleted entries and duplicates are
excluded. Nutrition and allergen fields are not collected or inferred.

The response includes the original source URLs, published month, fetch timestamp,
exact dated meals, published choices and a `dates_checked` flag. A failed
current-date lookup is distinguished from a successful lookup with no meals.
The browser renders food names as escaped text and only accepts an HTTPS viewer
link on the official menu host.

One shared, in-memory cache serves all six school selections. Refreshes are
serialized, expire after 30 minutes, and have an 18-second overall timeout.
A partial response retries after two minutes. Failures use a two-minute cooldown
and can return a clearly marked saved response for the same date for up to 24
hours. No browsing data, food preferences or feed records are persisted.

The lunch card follows the real calendar even when a bell schedule is being
previewed manually. On weekends, holidays and after dismissal it selects the
next school day. Exact meals take precedence; when absent, the card shows the
published month beside the available choices and retains the full-menu link.

## Verification

The Rust parser tests use inline synthetic meals and dates. Browser tests cover
exact-day selection, dismissal rollover, school switching, older publication
labels, unavailable service, unsafe remote names/URLs, themes and small screens.
To verify the upstream source explicitly:

```sh
cargo test --manifest-path rust/Cargo.toml -p mitch-server live_published_lunch_feed -- --ignored --nocapture
```

The live check is ignored during ordinary workspace tests so CI does not depend
on the district or menu provider being online.
