# X search — FxEmbed mirrors (our X leg)

Our X search does **not** use a Worker we own. It uses FxEmbed's public mirrors, the same ones `search/web.py` uses. There is no X Worker source to deploy here; this folder holds the manual and the base URLs so the setup is reproducible.

## Base URLs

Primary: `https://fixtweet.twapiworker.workers.dev`
Fallback: `https://fixtweet-2.twapiworker.workers.dev`

These are FxEmbed's public mirrors, not our infrastructure. If they go down, X search degrades to the TinyFish + Bing legs of `search/web.py research`.

## Rules

- A **User-Agent header is mandatory** — requests without one get 401.
- `search/web.py` (`xsearch`, `xpost`, `xprofile`, `xtrends`) already implements the calls; `research` includes the X ask pack.
- `fxembed-llms.txt` (in this folder and in `search/`) is the FxEmbed manual.

## What not to do

Do not "fix" X search by adding a paid X API key or a new X Worker unless the user asks. The FxEmbed mirrors are the agreed X leg.
