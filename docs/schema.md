# Document schema (Loro)

Top-level entries are Loro **root containers** (deterministic ids, so every
peer addresses the same container). Every nested container is created with
`ensure_mergeable_*`, so two peers creating "the same" child concurrently
merge instead of one replacing the other.

```
meta: LoroMap            name, fps_num, fps_den, width, height, sample_rate,
                         color_space, schema_version, main_seq
media: LoroMap<MediaId(hex blake3) → Map>
                         name, kind (video|audio|image), duration_ticks,
                         fps_num, fps_den, w, h, audio_ch, audio_rate,
                         size_bytes, proxy_hash?
bins: LoroTree           (folders; MVP)
sequences: LoroMap<SeqId(ulid) → Map>
  name
  tracks: LoroMovableList<TrackId>          order (V1 is the first visual track)
  track_props: Map<TrackId → Map>           kind, name, locked, hidden, muted, height
  clips: Map<ClipId(ulid) → Map>            flat LWW fields:
      name, track_id, start_tick, in_tick, out_tick,
      source_kind (media|title|color|adjustment), media_id?, color_rgba?,
      reversed, enabled, pos_clock, pos_peer, group_id?, link_id?
      speed:     Map { mode, num, den, ramp_curve? }      (exact rational)
      transform: Map { x, y, scale_x, scale_y, rotation, anchor_x, anchor_y,
                       opacity, blend, flip_h, flip_v }
      audio:     Map { gain_db, pan, fade_in, fade_out, duck }
      effects:   LoroMovableList<Map>                     (v1)
      keyframes: Map<param_path → List>                   (v1)
  transitions: Map<TransId → Map>                         (MVP)
  markers: Map<MarkerId → Map>  tick, color, name, comment, author, resolved
title_text: LoroMap<ClipId → LoroText>                    (MVP)
```

All times are `i64` flicks. The document never contains file paths.
