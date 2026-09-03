# Screenshots

Taken against a demo board of invented pins, never a real one. Nothing here is
anybody's actual notes.

| File | Shows | Theme |
|---|---|---|
| `cluster-zoom.png` | The most zoomed-out rung: solid colour blocks, no text. You navigate by colour and position | Catppuccin Mocha |
| `preview-zoom.png` | Title plus body preview, pins overlapping the way a real board looks | Catppuccin Mocha |
| `document-zoom.png` | Full notes, readable. The purple pin is longer than itself and shows the scroll thumb on its right border | Catppuccin Mocha |
| `keys-overlay.png` | `?` over a board at titles zoom | Nord |
| `keys-overlay-cluster.png` | The same list over colour blocks | Nord |

Retina captures, roughly 1950px wide, metadata stripped.

To reshoot, point pinz at a throwaway board so none of your own pins can end up
in a picture:

```sh
PINZ_HOME=~/pinz-demo pinz --no-sync
```

`--no-sync` keeps it from touching git, so you can rearrange pins for the shot
and `git checkout .` afterwards.
