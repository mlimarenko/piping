//! MCP tool handlers (access control, CCTV, host SSH/VNC, inventory, REST bridge) and their crate-internal re-exports.
mod access_control;
mod cctv;
mod host_ssh;
mod host_vnc;
mod inventory;
mod rest_bridge;

pub(super) use access_control::handle_access_control_tool_call;
pub(super) use cctv::{handle_cctv_archive_tool_call, handle_cctv_snapshot_tool_call};
pub(super) use host_ssh::handle_host_ssh_tool_call;
pub(super) use host_vnc::handle_host_vnc_tool_call;
pub(super) use inventory::handle_inventory_tool_call;
pub(super) use rest_bridge::{handle_generic_rest_call, handle_scoped_rest_endpoint_tool_call};
