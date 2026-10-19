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
| APP-005 | A1, A2, A3, A4, A5, A6 | example:vespertine_js_node:app_lifecycle ; example:vespertine_js_node:layer_shell ; test:vespertine_js:a3_an_unresponsive_ui_loop_times_out_after_two_seconds ; test:vespertine_js:a3_a_loop_that_stops_while_the_caller_waits_reports_the_stopped_error ; test:vespertine_js:a_command_after_the_loop_is_gone_fails_with_the_not_running_error ; test:vespertine_js:commands_before_initialization_fail_with_the_uninitialized_error ; test:vespertine_js:tick_before_initialization_tells_the_caller_to_initialize_first ; test:vespertine_js:tick_reports_liveness_and_a_second_attachment_is_rejected ; test:vespertine_js:queued_commands_run_in_submission_order ; test:vespertine_js:a_prompt_whose_loop_stops_rejects_with_the_stopped_error ; test:vespertine_js:the_size_query_returns_the_viewport_in_logical_pixels ; test:vespertine_js:the_insets_query_reports_zero_safe_area_ime_and_effective_insets ; test:vespertine_js:fire_and_forget_commands_run_in_order_on_the_ui_thread ; test:vespertine_js:a_changing_batch_queues_one_repaint_and_an_equivalent_resend_queues_none ; test:vespertine_js:a_batch_before_initialization_is_applied_and_the_repaint_request_fails ; test:vespertine_js:a_batch_after_the_loop_stopped_is_applied_and_the_repaint_request_fails_as_not_running ; test:vespertine_js:the_default_window_is_800_by_600_and_centered_on_the_primary_display ; test:vespertine_js:the_title_size_and_flags_are_forwarded ; test:vespertine_js:fullscreen_true_opens_fullscreen ; test:vespertine_js:backgrounds_follow_the_kind_then_the_transparency_flag ; test:vespertine_js:the_minimum_size_applies_only_when_both_values_are_given ; test:vespertine_js:the_application_identity_becomes_the_window_app_id ; test:vespertine_js:a_layer_shell_surface_has_no_titlebar_and_opens_at_the_origin ; test:vespertine_js:options::tests::defaults_are_800_by_600_resizable_opaque_focused_and_shown ; test:vespertine_js:options::tests::transparent_without_a_kind_selects_the_transparent_background ; test:vespertine_js:options::tests::an_explicit_kind_wins_over_plain_transparency ; test:vespertine_js:options::tests::minimum_size_applies_only_when_both_values_are_given ; test:vespertine_js:options::tests::layer_shell_defaults ; test:vespertine_js:options::tests::layers_parse_and_unknown_values_fall_back_to_top ; test:vespertine_js:options::tests::edge_names_ignore_case_and_whitespace_and_unknown_names_add_nothing ; test:vespertine_js:options::tests::exclusive_zone_keeps_zero_negative_and_absent_distinct ; test:vespertine_js:options::tests::margins_of_other_lengths_are_ignored ; test:vespertine_js:options::tests::keyboard_interactivity_parses_and_unknown_values_are_none ; test:vespertine_js:options::tests::titlebar_button_fields_and_unknown_fields_are_accepted ; test:vespertine_js:options::tests::invalid_sizes_are_rejected ; test:vespertine_js:prompt::tests::defaults_select_a_single_file ; test:vespertine_js:prompt::tests::directories_only_turns_files_off_by_default ; test:vespertine_js:prompt::tests::explicitly_disabling_both_rejects ; test:vespertine_js:prompt::tests::both_enabled_rejects_where_mixed_selection_is_unsupported ; test:vespertine_js:prompt::tests::a_path_that_is_not_valid_unicode_rejects ; test:vespertine_js:prompt::tests::empty_selection_and_cancellation_resolve_to_none ; test:vespertine_js:prompt::tests::selected_paths_are_returned_as_strings ; test:vespertine_js:prompt::tests::multiple_and_prompt_label_are_forwarded ; test:vespertine_js:a_cancelled_prompt_resolves_to_null ; test:vespertine_js:an_empty_selection_resolves_to_null ; test:vespertine_js:selected_paths_resolve_to_an_array_of_strings_and_options_reach_the_platform ; test:vespertine_js:invalid_prompt_options_reject_without_opening_a_chooser ; test:vespertine_js:files_and_directories_together_reject_where_the_platform_cannot_mix_them ; test:vespertine_js_node:javascript_tests_pass_under_node ; test:vespertine_js_node:javascript_tests_pass_under_bun |
| APP-006 | A1, A2, A3, A4, A5, A6, A7 | test:vespertine_host_tree:a1_create_and_append_builds_ordered_tree ; test:vespertine_host_tree:a1_reparent_moves_child_out_of_old_parent ; test:vespertine_host_tree:a1_insert_before_orders_and_missing_sibling_appends ; test:vespertine_host_tree:a1_destroy_subtree_returns_preorder_and_clears_root ; test:vespertine_host_tree:a1_destroy_missing_id_returns_nothing ; test:vespertine_host_tree:a1_repeated_equivalent_commits_leave_dirty_flag_clear ; test:vespertine_host_tree:a2_invalid_final_operation_leaves_earlier_operations_unapplied ; test:vespertine_host_tree:a2_malformed_final_style_leaves_earlier_operations_unapplied ; test:vespertine_host_tree:a2_decode_failures_precede_style_failures_and_report_first_failing_index ; test:vespertine_host_tree:a3_invalid_identifiers_rejected_in_every_position ; test:vespertine_host_tree:a3_boundary_identifiers_accepted ; test:vespertine_host_tree:a4_trailing_arguments_are_ignored ; test:vespertine_host_tree:a4_has_handler_must_be_boolean ; test:vespertine_host_tree:a4_operations_on_missing_elements_are_harmless ; test:vespertine_host_tree:a5_equivalent_style_takes_no_revision ; test:vespertine_host_tree:a5_search_revision_follows_text_and_structure_only ; test:vespertine_host_tree:a5_highlight_declaration_presence_changes_ancestor_search_revision ; test:vespertine_host_tree:a6_null_custom_prop_removes_a_property_and_resets_native_behavior ; test:vespertine_host_tree:a6_auto_focus_and_test_id_are_not_general_props ; test:vespertine_host_tree:a6_focusable_and_animated_indices_follow_listener_tabindex_and_motion_changes ; test:vespertine_host_tree:a7_collision_resolves_by_content ; test:vespertine_host_tree:a7_sweep_reclaims_styles_of_destroyed_and_restyled_elements ; test:vespertine_host_tree:a7_no_sweep_below_floor_and_sweep_at_thresholds ; test:vespertine_host_tree:a7_sweep_after_destroying_most_of_a_large_tree_leaves_at_most_live_count |
| APP-007 | A1, A2, A3, A4 | test:vespertine_js_node:javascript_tests_pass_under_node ; test:vespertine_js_node:javascript_tests_pass_under_bun |
| APP-008 | A1, A2, A3 | test:vespertine_js_node:javascript_tests_pass_under_node ; test:vespertine_js_node:javascript_tests_pass_under_bun |

## A11Y

Accessibility.

| Spec | Acceptance | Verified by |
|---|---|---|

## DEV

Developer tooling: inspector, profiling, debugging, hot reload.

| Spec | Acceptance | Verified by |
|---|---|---|
| DEV-002 | A1, A2, A3, A4, A5 | test:vespertine_wgpu:wgpu_renderer::recording_tests::memory_reports_follow_the_injected_clock_and_list_every_component ; test:vespertine_wgpu:memory_report::tests::reports_on_the_first_frame_then_every_ten_seconds ; test:vespertine_wgpu:memory_report::tests::disabled_reporters_never_evaluate_or_emit ; test:vespertine_wgpu:memory_report::tests::enable_values ; test:vespertine_wgpu:memory_report::tests::the_decision_is_read_once_and_cached ; test:vespertine_wgpu:memory_report::tests::atlas_estimate_counts_allocations_and_skips_empty_slots ; test:vespertine_wgpu:memory_report::tests::path_estimate_multiplies_the_multisample_target ; test:vespertine_wgpu:memory_report::tests::mib_formatting_has_one_decimal ; test:vespertine_wgpu:memory_report::tests::the_report_line_lists_fields_in_order ; example:vespertine:renderer_memory ; test:vespertine_wgpu:memory_report::tests::two_reporters_keep_independent_timers |
| DEV-008 | A1, A2, A3, A4 | example:vespertine_host_tree:mutation_import_bench |
