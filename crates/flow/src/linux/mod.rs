//! Linux desktops: KDE Plasma on Wayland (KWin script over D-Bus, keystate, data-control,
//! uinput absolute pointer, evdev button state, the edge catcher for file drags and the drag
//! watch that makes the edges glow while files are dragged; KDE drops that copy without
//! asking) and X11 (x11rb).

pub mod arkdrop;
pub mod catcher;
pub mod datacontrol;
pub mod dragwatch;
pub mod input;
pub mod keystate;
pub mod kwin;
pub mod wayland;
pub mod x11;
