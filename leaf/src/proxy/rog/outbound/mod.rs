#[cfg(feature = "outbound-rog")]
pub mod datagram;
#[cfg(feature = "outbound-rog")]
pub mod stream;

#[cfg(feature = "outbound-rog")]
pub use datagram::Handler as DatagramHandler;
#[cfg(feature = "outbound-rog")]
pub use stream::Handler as StreamHandler;
