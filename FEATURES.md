# Features

Every implemented specification is listed here, under its area, with the acceptance items it
covers and what verifies them. `./scripts/check-features` parses these tables, checks that every
verifier exists, runs them, and fails if any row fails; it is part of the landing gates.

Row syntax (one spec per row, rows in ascending spec order):

```
| <SPEC-ID> | A1, A2, ... | test:<crate>:<test name> ; example:<crate>:<example name> |
```

- `test:<crate>:<name>` is a test exactly as printed by `cargo test -p <crate> -- --list`
  (without the `: test` suffix).
- `example:<crate>:<name>` is a cargo example of the crate. It is built in release mode and run
  for 10 seconds in the current graphical session; it passes if it exits 0 or is still running
  at the timeout.
- Verifiers are separated by ` ; `. A row lists every acceptance item (`A1`, `A2`, ...) of its spec.

Visual and performance scenes (`FID-*`, `PERF-*`) live in `crates/vespertine_scenes`; see its README.

## RND

GPU renderer, compositing, shaders, visual effects, color handling.

| Spec | Acceptance | Verified by |
|---|---|---|
| RND-003 | A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11, A12, A13, A14, A15, A16 | test:vespertine_wgpu:wgpu_renderer::blur_tests::blur_snapshots_earlier_content_and_leaves_later_content_sharp ; test:vespertine_wgpu:wgpu_renderer::blur_tests::overlapping_effects_chain_and_each_uses_its_own_sigma ; test:vespertine_wgpu:wgpu_renderer::blur_tests::constant_input_stays_constant_including_at_every_edge ; test:vespertine_wgpu:wgpu_renderer::blur_tests::linear_ramp_keeps_its_position_for_non_divisible_snapshots ; test:vespertine_wgpu:wgpu_renderer::blur_tests::rounded_edge_is_a_hard_cutoff_and_mask_clips_the_effect ; test:vespertine_wgpu:wgpu_renderer::blur_tests::direct_and_intermediate_presentation_routes_match_and_keep_alpha ; test:vespertine_wgpu:wgpu_renderer::blur_tests::effect_free_frames_do_not_use_the_presentation_intermediate ; test:vespertine_wgpu:wgpu_renderer::blur_tests::only_the_first_thirty_two_effects_render ; test:vespertine_wgpu:wgpu_renderer::blur_tests::a_fully_clipped_effect_does_not_damage_earlier_pixels ; test:vespertine_wgpu:blur::tests::filter_parameters_match_spec_examples ; test:vespertine_wgpu:blur::tests::envelope_padding_for_sigma_16_is_fifty ; test:vespertine_wgpu:blur::tests::weights_are_normalized_and_zero_beyond_radius ; example:vespertine:backdrop_blur ; test:vespertine:backdrop_blur_tests::effect_and_ordering_primitive_are_scaled_by_the_scale_factor ; test:vespertine:backdrop_blur_tests::zero_and_negative_radius_emit_nothing ; test:vespertine:backdrop_blur_tests::the_content_mask_clips_the_effect_and_drops_fully_clipped_ones ; test:vespertine_wgpu:wgpu_renderer::blur_tests::four_distinct_corner_radii_produce_four_distinct_corners ; test:vespertine_wgpu:wgpu_renderer::blur_tests::nested_panels_in_separate_layers_see_earlier_panels_but_not_later_siblings ; test:vespertine_wgpu:wgpu_renderer::blur_tests::boundary_primitive_alone_is_invisible_and_a_trailing_effect_still_renders ; test:vespertine_wgpu:wgpu_renderer::blur_tests::the_card_is_pixel_identical_after_an_unrelated_repaint ; test:vespertine_wgpu:wgpu_renderer::blur_tests::a_nested_overlay_layer_draws_above_the_card_even_for_earlier_primitive_kinds ; test:vespertine:backdrop_blur_tests::replaying_a_retained_range_reproduces_the_effect ; test:vespertine:backdrop_blur_tests::a_layer_shares_one_order_and_nested_layers_draw_above_it ; test:vespertine:backdrop_blur_tests::batches_are_cut_at_effect_orders ; test:vespertine_wgpu:wgpu_renderer::blur_tests::opaque_styling_renders_without_translucency_and_the_blur_stays_available |
| RND-006 | A1, A2, A3, A4, A5, A6 | example:vespertine:gpu_surface ; test:vespertine:elements::surface::tests::surface_bounds_are_scaled_and_snapped_to_device_pixels ; test:vespertine:elements::surface::tests::surface_defaults_to_contain_and_other_fits_follow_images ; test:vespertine:elements::surface::tests::surface_is_clipped_by_ancestor_mask_and_own_bounds ; test:vespertine:elements::surface::tests::scene_retains_texture_until_it_is_dropped ; test:vespertine_wgpu:wgpu_surfaces::tests::wrong_type_textures_are_skipped_without_side_effects ; test:vespertine_wgpu:wgpu_surfaces::tests::fid_rnd_006_two_surfaces_keep_their_own_bounds_and_clips ; test:vespertine_wgpu:wgpu_surfaces::tests::device_loss_and_recovery_present_a_surface_again ; test:vespertine_wgpu:wgpu_surfaces::tests::context_request_during_loss_returns_none ; test:vespertine_wgpu:wgpu_surfaces::tests::surfaces_keep_z_order_with_other_primitives ; test:vespertine_wgpu:wgpu_surfaces::tests::surface_path_has_zero_readback_and_zero_image_upload |

## TXT

Fonts, shaping, rasterization, text layout and text rendering.

| Spec | Acceptance | Verified by |
|---|---|---|

## LAY

Layout engine and layout styles.

| Spec | Acceptance | Verified by |
|---|---|---|

## ELM

Elements and the styling API.

| Spec | Acceptance | Verified by |
|---|---|---|

## CMP

Reusable components and widgets built from elements.

| Spec | Acceptance | Verified by |
|---|---|---|

## INP

Keyboard, mouse, touchpad, IME, focus, actions, drag and drop.

| Spec | Acceptance | Verified by |
|---|---|---|

## PLT

Linux platform integration.

| Spec | Acceptance | Verified by |
|---|---|---|

## ANM

Animation, timing, frame scheduling.

| Spec | Acceptance | Verified by |
|---|---|---|

## THM

Theming, design tokens, color systems.

| Spec | Acceptance | Verified by |
|---|---|---|

## PRF

Performance: caching, batching, allocation, scheduling, threading.

| Spec | Acceptance | Verified by |
|---|---|---|
| PRF-007 | A1, A2, A3, A4, A5 | test:vespertine_wgpu:wgpu_renderer::recording_tests::grouped_path_rasterization_matches_the_per_batch_baseline ; test:vespertine_wgpu:wgpu_renderer::recording_tests::a_frame_uploads_its_instance_data_once_and_globals_only_when_changed ; test:vespertine_wgpu:wgpu_renderer::recording_tests::growth_replaces_the_buffer_without_aliasing_old_bindings ; test:vespertine_wgpu:wgpu_renderer::recording_tests::unused_bindings_are_released_after_two_frames ; test:vespertine_wgpu:wgpu_renderer::recording_tests::separated_path_batches_share_one_rasterization_pass ; test:vespertine_wgpu:wgpu_renderer::recording_tests::a_hundred_thirty_separated_batches_form_three_groups ; test:vespertine_wgpu:wgpu_renderer::recording::tests::redundant_state_commands_are_suppressed_and_pass_restarts_reset_tracking |
| PRF-009 | A1, A2, A3, A4, A5, A6, A7, A8 | test:vespertine_wgpu:wgpu_renderer::blur_tests::blur_storage_is_reused_replaced_and_released ; test:vespertine_wgpu:wgpu_renderer::blur_tests::a_reused_larger_allocation_produces_the_same_image_as_a_fresh_one ; test:vespertine_wgpu:blur::tests::weights_are_normalized_and_zero_beyond_radius ; test:vespertine_wgpu:blur::tests::oversized_radius_skips_the_table ; test:vespertine_wgpu:wgpu_renderer::blur_tests::sigma_beyond_the_table_uses_the_full_support_fallback ; test:vespertine_wgpu:blur::tests::paired_and_unpaired_agree_including_edges ; test:vespertine_wgpu:blur::tests::odd_radius_final_partner_has_zero_weight ; test:vespertine_wgpu:blur::tests::scissors_cover_composite_neighbors_and_vertical_taps ; test:vespertine_wgpu:blur::tests::scissors_are_translation_invariant ; test:vespertine_wgpu:blur::tests::zero_area_scissor_has_end_at_least_start ; test:vespertine_wgpu:wgpu_renderer::blur_tests::narrow_regions_shade_fewer_texels_than_full_targets ; test:vespertine_wgpu:blur::tests::narrow_regions_shade_fewer_texels_than_the_full_target ; test:vespertine_wgpu:wgpu_renderer::blur_tests::effect_free_frames_do_not_use_the_presentation_intermediate ; test:vespertine_wgpu:wgpu_renderer::blur_tests::direct_and_intermediate_presentation_routes_match_and_keep_alpha |
| PRF-010 | A1, A2, A3, A4, A5, A6 | test:vespertine_wgpu:wgpu_renderer::recording_tests::path_intermediates_are_created_on_demand_and_released_after_inactivity ; test:vespertine_wgpu:wgpu_renderer::recording_tests::parking_after_one_path_free_frame_releases_path_intermediates ; test:vespertine_wgpu:wgpu_renderer::recording_tests::instance_buffer_shrinks_after_sustained_low_usage_and_never_below_the_floor ; test:vespertine_wgpu:wgpu_renderer::recording_tests::a_frame_above_half_capacity_restarts_the_shrink_streak ; test:vespertine_wgpu:wgpu_renderer::recording_tests::frames_after_a_shrink_render_identical_pixels ; test:vespertine_wgpu:wgpu_renderer::blur_tests::trim_releases_blur_storage_only_after_an_effect_free_frame ; test:vespertine_wgpu:wgpu_renderer::blur_tests::blur_storage_is_reused_replaced_and_released |
| PRF-016 | A1, A2, A3, A4, A5 | test:vespertine_wgpu:image_update_tests::same_size_update_keeps_the_tile_and_changes_pixels_then_resize_reallocates ; test:vespertine_wgpu:image_update_tests::update_of_a_missing_key_inserts_a_tile ; test:vespertine:window::image_update_tests::multi_frame_update_is_retained_only_when_every_frame_is ; test:vespertine:window::image_update_tests::missing_frame_tile_is_not_retained ; test:vespertine:assets::tests::update_image_frames_requires_every_frame_to_be_retained ; test:vespertine:assets::tests::update_image_frames_is_false_for_zero_frames ; test:vespertine:assets::tests::update_image_frames_propagates_invalid_frame_access ; test:vespertine:window::image_update_tests::cached_presentation_after_retained_updates_skips_the_element_tree ; test:vespertine:window::image_update_tests::cached_presentation_falls_back_to_a_redraw ; example:vespertine:dynamic_image ; test:vespertine_wgpu:image_update_tests::queue_writes_are_ordered_against_frames_sampling_the_texture ; test:vespertine_wgpu:image_update_tests::update_with_mismatched_bytes_errors_and_keeps_the_existing_tile ; test:vespertine:platform::atlas_tests::same_size_update_replaces_in_place_after_waiting_for_the_gpu ; test:vespertine:platform::atlas_tests::size_change_uses_remove_then_insert |

## APP

Application runtime: entities, contexts, executors, actions, menus.

| Spec | Acceptance | Verified by |
|---|---|---|

## A11Y

Accessibility.

| Spec | Acceptance | Verified by |
|---|---|---|

## DEV

Developer tooling: inspector, profiling, debugging, hot reload.

| Spec | Acceptance | Verified by |
|---|---|---|
| DEV-002 | A1, A2, A3, A4, A5 | test:vespertine_wgpu:wgpu_renderer::recording_tests::memory_reports_follow_the_injected_clock_and_list_every_component ; test:vespertine_wgpu:memory_report::tests::reports_on_the_first_frame_then_every_ten_seconds ; test:vespertine_wgpu:memory_report::tests::disabled_reporters_never_evaluate_or_emit ; test:vespertine_wgpu:memory_report::tests::enable_values ; test:vespertine_wgpu:memory_report::tests::the_decision_is_read_once_and_cached ; test:vespertine_wgpu:memory_report::tests::atlas_estimate_counts_allocations_and_skips_empty_slots ; test:vespertine_wgpu:memory_report::tests::path_estimate_multiplies_the_multisample_target ; test:vespertine_wgpu:memory_report::tests::mib_formatting_has_one_decimal ; test:vespertine_wgpu:memory_report::tests::the_report_line_lists_fields_in_order ; example:vespertine:renderer_memory ; test:vespertine_wgpu:memory_report::tests::two_reporters_keep_independent_timers |
