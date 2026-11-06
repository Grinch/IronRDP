pub mod activation;
pub mod config;
pub mod result;
pub mod state;

use ironrdp::pdu::gcc;

/// The session the client asks for in GCC Client Cluster Data, built up by the `ClientConnector` methods that set it.
#[derive(Debug, Default, Clone, Copy)]
pub struct ClusterRequest {
    redirection_supported: bool,
    admin_session: bool,
    redirected_session_id: Option<u32>,
}

impl ClusterRequest {
    /// The Client Cluster Data to send ([MS-RDPBCGR] 2.2.1.3.5 "Client Cluster Data (TS_UD_CS_CLUSTER)"), if any.
    ///
    /// Any request advertises `REDIRECTION_SUPPORTED` with redirection version 5, as FreeRDP does: version 6 is for
    /// clients that support multitransport, which this one does not advertise.
    /// A session ID, with `REDIRECTED_SESSIONID_FIELD_VALID`, asks the server to connect to that session: the one a
    /// Server Redirection PDU named, or else 0 for the console (administrative) session, as `mstsc /admin` does.
    ///
    /// [MS-RDPBCGR]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/
    fn cluster_data(self) -> Option<gcc::ClientClusterData> {
        let session_id = self
            .redirected_session_id
            .or(if self.admin_session { Some(0) } else { None });
        if !self.redirection_supported && session_id.is_none() {
            return None;
        }

        let mut flags = gcc::RedirectionFlags::REDIRECTION_SUPPORTED;
        if session_id.is_some() {
            flags |= gcc::RedirectionFlags::REDIRECTED_SESSION_FIELD_VALID;
        }
        Some(gcc::ClientClusterData {
            flags,
            redirection_version: gcc::RedirectionVersion::V5,
            redirected_session_id: session_id.unwrap_or(0),
        })
    }
}

/// The routing token to send in the X.224 Connection Request, from the `LoadBalanceInfo` of a Server Redirection PDU
/// or the `loadbalanceinfo` of an RDP file.
///
/// The token is printable ASCII ended by CR LF on the wire ([MS-RDPBCGR] 2.2.1.1 "Client X.224 Connection Request
/// PDU"), so one trailing CR LF is dropped here and the connector writes it back.
///
/// [MS-RDPBCGR]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/
fn routing_token(load_balance_info: &[u8]) -> Result<String, &'static str> {
    let token = load_balance_info.strip_suffix(b"\r\n").unwrap_or(load_balance_info);
    if token.is_empty() || ironrdp::pdu::nego::MAX_ROUTING_TOKEN_LENGTH < token.len() {
        return Err("load balance info must hold 1 to 238 bytes before its CR LF");
    }
    if !token.iter().all(|byte| (0x20..=0x7E).contains(byte)) {
        return Err("load balance info must be printable ASCII");
    }
    String::from_utf8(token.to_vec()).map_err(|_| "load balance info must be printable ASCII")
}

#[diplomat::bridge]
pub mod ffi {
    use core::fmt::Write as _;

    use diplomat_runtime::DiplomatWrite;
    use ironrdp::connector::Sequence as _;
    use ironrdp::displaycontrol::client::DisplayControlClient;
    use ironrdp::dvc::DvcClientProcessor;
    use ironrdp::rdpei::RdpeiClient;
    use ironrdp::rdpei::pdu::{CsReadyFlags, RdpInputProtocolVersion};
    use ironrdp_dvc_pipe_proxy::DvcNamedPipeProxy;
    use tracing::info;

    use super::config::ffi::Config;
    use super::result::ffi::Written;
    use super::state::ffi::ClientConnectorState;
    use crate::clipboard::ffi::Cliprdr;
    use crate::dvc::dvc_pipe_proxy_message_queue::DvcPipeProxyMessageInner;
    use crate::dvc::ffi::DvcPipeProxyConfig;
    use crate::error::ValueConsumedError;
    use crate::error::ffi::{IronRdpError, IronRdpErrorKind};
    use crate::pdu::ffi::WriteBuf;
    use crate::utils::monotonic_now;

    #[diplomat::opaque_mut] // We must use Option here, as ClientConnector is not Clone and have functions that consume it
    pub struct ClientConnector(
        pub Option<ironrdp::connector::ClientConnector>,
        /// What the cluster data methods asked for so far, so each one keeps what the others set.
        pub super::ClusterRequest,
    );

    // Basic Impl for ClientConnector
    impl ClientConnector {
        pub fn new(config: &Config, client_addr: &str) -> Result<Box<ClientConnector>, Box<IronRdpError>> {
            let client_addr = client_addr.parse().map_err(|_| IronRdpErrorKind::Generic)?;

            Ok(Box::new(ClientConnector(
                Some(ironrdp::connector::ClientConnector::new(
                    config.connector.clone(),
                    client_addr,
                )),
                super::ClusterRequest::default(),
            )))
        }

        /// Replaces the GCC Client Cluster Data with the one `request` describes.
        fn set_cluster_request(&mut self, request: super::ClusterRequest) -> Result<(), Box<IronRdpError>> {
            let Some(connector) = self.0.take() else {
                return Err(ValueConsumedError::for_item("connector").into());
            };
            self.0 = Some(match request.cluster_data() {
                Some(cluster_data) => connector.with_cluster_data(cluster_data),
                None => connector,
            });
            self.1 = request;
            Ok(())
        }

        /// Tells the server that this client follows Server Redirection PDUs ([MS-RDPBCGR] 2.2.13.1), by sending
        /// GCC Client Cluster Data with `REDIRECTION_SUPPORTED`.
        ///
        /// A Connection Broker redirects only clients that say so.
        /// A redirected connection ends in `ClientConnectorState` or `ConnectionActivationState` `Redirected`, whose
        /// `ServerRedirection` names the target; connect again with `with_load_balance_info`,
        /// `with_redirected_session_id` and `with_redirection_password_cookie`.
        /// `with_admin_session` and `with_redirected_session_id` advertise redirection support as well.
        ///
        /// [MS-RDPBCGR]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/
        pub fn with_redirection_support(&mut self) -> Result<(), Box<IronRdpError>> {
            self.set_cluster_request(super::ClusterRequest {
                redirection_supported: true,
                ..self.1
            })
        }

        /// Asks for the console session, as `mstsc /admin` does, which on Windows Server is the administrative
        /// session that needs no Remote Desktop Services client access license.
        ///
        /// GCC Client Cluster Data ([MS-RDPBCGR] 2.2.1.3.5) carries `REDIRECTED_SESSIONID_FIELD_VALID` with session ID
        /// 0, and `REDIRECTION_SUPPORTED` with redirection version 5, as FreeRDP sends for `/admin`.
        /// A session ID from `with_redirected_session_id` takes precedence over 0.
        ///
        /// [MS-RDPBCGR]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/
        pub fn with_admin_session(&mut self) -> Result<(), Box<IronRdpError>> {
            self.set_cluster_request(super::ClusterRequest {
                admin_session: true,
                ..self.1
            })
        }

        /// Asks for the session a Server Redirection PDU named (`ServerRedirection.session_id`), when reconnecting
        /// after a redirection ([MS-RDPBCGR] 3.1.5.1.1).
        ///
        /// GCC Client Cluster Data carries `REDIRECTED_SESSIONID_FIELD_VALID` with `session_id`, and
        /// `REDIRECTION_SUPPORTED`.
        /// It takes precedence over the session ID 0 of `with_admin_session`.
        ///
        /// [MS-RDPBCGR]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/
        pub fn with_redirected_session_id(&mut self, session_id: u32) -> Result<(), Box<IronRdpError>> {
            self.set_cluster_request(super::ClusterRequest {
                redirected_session_id: Some(session_id),
                ..self.1
            })
        }

        /// Sends `load_balance_info` as the routing token of the X.224 Connection Request ([MS-RDPBCGR] 2.2.1.1),
        /// in place of the user name cookie.
        ///
        /// Pass the `ServerRedirection.load_balance_info` of a redirection ([MS-RDPBCGR] 2.2.13.1), or the
        /// `loadbalanceinfo` of an RDP file as bytes.
        /// One trailing CR LF is dropped, since the connector ends the token with one.
        /// Fails unless the rest is 1 to 238 bytes of printable ASCII, the only routing token the connector can send;
        /// a Connection Broker's token (`Cookie: msts=...`) is.
        ///
        /// [MS-RDPBCGR]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/
        pub fn with_load_balance_info(&mut self, load_balance_info: &[u8]) -> Result<(), Box<IronRdpError>> {
            let token = super::routing_token(load_balance_info)?;
            let Some(connector) = self.0.take() else {
                return Err(ValueConsumedError::for_item("connector").into());
            };
            self.0 = Some(connector.with_load_balance_info(token));
            Ok(())
        }

        /// Sends `cookie`, the `ServerRedirection.password` of a redirection, as the password of the Client Info
        /// PDU ([MS-RDPBCGR] 2.2.13.1).
        ///
        /// The configured credentials still authenticate CredSSP.
        ///
        /// [MS-RDPBCGR]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/
        pub fn with_redirection_password_cookie(&mut self, cookie: &[u8]) -> Result<(), Box<IronRdpError>> {
            if cookie.is_empty() {
                return Err("redirection password cookie is empty".into());
            }
            let Some(connector) = self.0.take() else {
                return Err(ValueConsumedError::for_item("connector").into());
            };
            self.0 = Some(connector.with_redirection_password_cookie(cookie.to_vec()));
            Ok(())
        }

        // FIXME: Naming: since this is not a builder pattern, use "attach"?
        // FIXME: We need to create opaque for ironrdp::svc::StaticChannelSet
        /// Must use
        pub fn with_static_channel_rdp_snd(&mut self) -> Result<(), Box<IronRdpError>> {
            use ironrdp::rdpsnd::client::{NoopRdpsndBackend, Rdpsnd};

            let Some(connector) = self.0.take() else {
                return Err(IronRdpErrorKind::Consumed.into());
            };

            self.0 = Some(connector.with_static_channel(Rdpsnd::new(Box::new(NoopRdpsndBackend {}))));

            Ok(())
        }

        // FIXME: We need to create opaque for ironrdp::rdpdr::Rdpdr
        /// Must use
        ///
        /// Announces a smartcard device. On Windows, calls are passed through to the local
        /// WinSCard service; elsewhere every call fails with `SCARD_E_NO_SERVICE`.
        ///
        /// Some WinSCard calls complete asynchronously: call `ActiveStage::poll_rdpdr_deferred`
        /// periodically to send their results to the server.
        pub fn with_static_channel_rdpdr(
            &mut self,
            computer_name: &str,
            smart_card_device_id: u32,
        ) -> Result<(), Box<IronRdpError>> {
            let Some(connector) = self.0.take() else {
                return Err(ValueConsumedError::for_item("connector").into());
            };

            #[cfg(windows)]
            let backend: Box<dyn ironrdp::rdpdr::RdpdrBackend> = Box::new(
                ironrdp_rdpdr_native::WindowsRdpdrBackendFactory::from_drives(Vec::new())
                    .map_err(|error| {
                        crate::error::GenericError(
                            anyhow::Error::new(error).context("invalid RDPDR backend configuration"),
                        )
                    })?
                    .with_smartcard(true)
                    .build(),
            );
            #[cfg(not(windows))]
            let backend: Box<dyn ironrdp::rdpdr::RdpdrBackend> = Box::new(ironrdp::rdpdr::NoopRdpdrBackend);

            self.0 = Some(connector.with_static_channel(
                ironrdp::rdpdr::Rdpdr::new(backend, computer_name.to_owned()).with_smartcard(smart_card_device_id),
            ));

            Ok(())
        }

        fn with_dvc<T>(&mut self, processor: T) -> Result<(), Box<IronRdpError>>
        where
            T: DvcClientProcessor + 'static,
        {
            let Some(connector) = &mut self.0 else {
                return Err(ValueConsumedError::for_item("connector").into());
            };

            let drdynvc = match connector.get_static_channel_processor_mut::<ironrdp::dvc::DrdynvcClient>() {
                Some(processor) => processor,
                None => {
                    connector.attach_static_channel(ironrdp::dvc::DrdynvcClient::new());
                    connector
                        .get_static_channel_processor_mut::<ironrdp::dvc::DrdynvcClient>()
                        .expect("DrdynvcClient should be initialized above")
                }
            };

            drdynvc.attach_dynamic_channel(processor);

            Ok(())
        }

        pub fn with_dynamic_channel_display_control(&mut self) -> Result<(), Box<IronRdpError>> {
            self.with_dvc(DisplayControlClient::new(|c| {
                info!(display_control_capabilities = ?c, "DisplayControl capabilities received");
                Ok(Vec::new())
            }))
        }

        /// Attaches the MS-RDPEI dynamic channel so the client can send touch frames.
        ///
        /// `max_touch_contacts` is the `maxTouchContacts` advertised in `RDPINPUT_CS_READY_PDU`
        /// ([MS-RDPEI] 2.2.3.2 "Client Ready PDU") and is the limit to give `TouchFrameBuilder`.
        /// `show_touch_visuals` asks the server to draw its touch visuals.
        /// Protocol version 2 is advertised, which covers touch and pen frames; multipen injection is
        /// not requested, so pen frames use `deviceId` 0.
        ///
        /// [MS-RDPEI]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpei/deb1ca39-344a-467a-b9d4-dfe196b46c9d
        pub fn with_dynamic_channel_rdpei(
            &mut self,
            max_touch_contacts: u16,
            show_touch_visuals: bool,
        ) -> Result<(), Box<IronRdpError>> {
            let flags = if show_touch_visuals {
                CsReadyFlags::SHOW_TOUCH_VISUALS
            } else {
                CsReadyFlags::empty()
            };
            self.with_dvc(RdpeiClient::new(
                max_touch_contacts,
                flags,
                RdpInputProtocolVersion::V200,
            ))
        }

        pub fn with_dynamic_channel_pipe_proxy(
            &mut self,
            config: &DvcPipeProxyConfig,
        ) -> Result<(), Box<IronRdpError>> {
            for descriptor in &config.descriptors {
                let sink = config.message_sink.0.clone();
                let proxy = DvcNamedPipeProxy::new(
                    &descriptor.channel_name,
                    &descriptor.pipe_name,
                    move |channel_id, svc_message| {
                        let _ = sink.send(DvcPipeProxyMessageInner(channel_id, svc_message));
                        Ok(())
                    },
                );
                self.with_dvc(proxy)?;
            }
            Ok(())
        }

        pub fn should_perform_security_upgrade(&self) -> Result<bool, Box<IronRdpError>> {
            let Some(connector) = self.0.as_ref() else {
                return Err(ValueConsumedError::for_item("connector").into());
            };
            Ok(connector.should_perform_security_upgrade())
        }

        pub fn mark_security_upgrade_as_done(&mut self) -> Result<(), Box<IronRdpError>> {
            let Some(connector) = self.0.as_mut() else {
                return Err(ValueConsumedError::for_item("connector").into());
            };
            connector.mark_security_upgrade_as_done();
            Ok(())
        }

        pub fn should_perform_credssp(&self) -> Result<bool, Box<IronRdpError>> {
            let Some(connector) = self.0.as_ref() else {
                return Err(ValueConsumedError::for_item("connector").into());
            };

            Ok(connector.should_perform_credssp())
        }

        pub fn mark_credssp_as_done(&mut self) -> Result<(), Box<IronRdpError>> {
            let Some(connector) = self.0.as_mut() else {
                return Err(ValueConsumedError::for_item("connector").into());
            };
            connector.mark_credssp_as_done();
            Ok(())
        }

        /// Send X.224 with an explicit protocol set (VMConnect uses HYBRID only).
        pub fn initiate_with_security_protocol(
            &mut self,
            security_protocol: u32,
            write_buf: &mut WriteBuf,
        ) -> Result<Box<Written>, Box<IronRdpError>> {
            let Some(connector) = self.0.as_mut() else {
                return Err(ValueConsumedError::for_item("connector").into());
            };
            let security_protocol = ironrdp::pdu::nego::SecurityProtocol::from_bits(security_protocol)
                .ok_or_else(|| ironrdp::connector::general_err!("invalid security protocol"))?;
            let written = connector.initiate_with_security_protocol(security_protocol, &mut write_buf.0)?;
            Ok(Box::new(Written(written)))
        }

        /// Drop host identity after pre-X.224 CredSSP so it is not forwarded into guest RDP.
        pub fn clear_credentials_after_host_auth(&mut self) -> Result<(), Box<IronRdpError>> {
            let Some(connector) = self.0.as_mut() else {
                return Err(ValueConsumedError::for_item("connector").into());
            };
            connector.config.credentials = ironrdp::connector::Credentials::UsernamePassword {
                username: String::new(),
                password: String::new(),
            };
            connector.config.domain = None;
            connector.config.autologon = false;
            Ok(())
        }

        /// Require EnhancedSecurityUpgrade with HYBRID, matching ironrdp-vmconnect::connect_front.
        pub fn ensure_selected_hybrid(&self) -> Result<(), Box<IronRdpError>> {
            let Some(connector) = self.0.as_ref() else {
                return Err(ValueConsumedError::for_item("connector").into());
            };
            ironrdp_vmconnect::ensure_selected_credssp(&connector.state).map_err(Into::into)
        }

        pub fn step(&mut self, input: &[u8], write_buf: &mut WriteBuf) -> Result<Box<Written>, Box<IronRdpError>> {
            let Some(connector) = self.0.as_mut() else {
                return Err(ValueConsumedError::for_item("connector").into());
            };
            // The FFI surface has no parameter through which the .NET caller could pass
            // when it read `input`, so this stamps on entry to the call instead. That is
            // at least as accurate as the other drivers' `last_read_at`, which is also a
            // post-read stamp taken right after the read completes rather than exactly
            // when the first byte arrived.
            let written = connector.step(input, Some(monotonic_now()), &mut write_buf.0)?;
            Ok(Box::new(Written(written)))
        }

        pub fn step_no_input(&mut self, write_buf: &mut WriteBuf) -> Result<Box<Written>, Box<IronRdpError>> {
            let Some(connector) = self.0.as_mut() else {
                return Err(ValueConsumedError::for_item("connector").into());
            };
            let written = connector.step_no_input(&mut write_buf.0)?;
            Ok(Box::new(Written(written)))
        }

        pub fn attach_static_cliprdr(&mut self, cliprdr: &mut Cliprdr) -> Result<(), Box<IronRdpError>> {
            let Some(connector) = self.0.as_mut() else {
                return Err(ValueConsumedError::for_item("connector").into());
            };

            let Some(cliprdr) = cliprdr.0.take() else {
                return Err(ValueConsumedError::for_item("cliprdr").into());
            };

            connector.attach_static_channel(cliprdr);
            Ok(())
        }

        pub fn next_pdu_hint<'a>(&'a self) -> Result<Option<Box<PduHint<'a>>>, Box<IronRdpError>> {
            let Some(connector) = self.0.as_ref() else {
                return Err(ValueConsumedError::for_item("connector").into());
            };
            tracing::trace!(pdu_hint=?connector.next_pdu_hint(), "Reading next PDU hint");
            Ok(connector.next_pdu_hint().map(PduHint).map(Box::new))
        }

        #[diplomat::attr(auto, getter = "dyn_state")]
        pub fn get_dyn_state<'a>(&'a self) -> Result<Box<DynState<'a>>, Box<IronRdpError>> {
            let Some(connector) = self.0.as_ref() else {
                return Err(ValueConsumedError::for_item("connector").into());
            };
            Ok(Box::new(DynState(connector.state())))
        }

        pub fn consume_and_cast_to_client_connector_state(
            &mut self,
        ) -> Result<Box<ClientConnectorState>, Box<IronRdpError>> {
            let Some(connector) = self.0.take() else {
                return Err(ValueConsumedError::for_item("connector").into());
            };
            Ok(Box::new(ClientConnectorState(Some(connector.state))))
        }
    }

    #[diplomat::opaque]
    pub struct PduHint<'a>(pub &'a dyn ironrdp::pdu::PduHint);

    impl<'a> PduHint<'a> {
        pub fn find_size(&'a self, bytes: &[u8]) -> Result<Box<crate::utils::ffi::OptionalUsize>, Box<IronRdpError>> {
            let pdu_hint = self.0;
            // TODO C# NuGet is only used on client-side so we probably don’t need to break the ABI for that just now.
            let size = pdu_hint.find_size(bytes)?.map(|(_match, size)| size);
            Ok(Box::new(crate::utils::ffi::OptionalUsize(size)))
        }
    }

    #[diplomat::opaque]
    pub struct DynState<'a>(pub &'a dyn ironrdp::connector::State);

    impl<'a> DynState<'a> {
        #[diplomat::attr(auto, getter = "name")]
        pub fn get_name(&'a self, writeable: &'a mut DiplomatWrite) -> Result<(), Box<IronRdpError>> {
            let name = self.0.name();
            write!(writeable, "{name}")?;
            Ok(())
        }

        pub fn is_terminal(&'a self) -> bool {
            self.0.is_terminal()
        }
    }

    #[diplomat::opaque]
    pub struct ChannelConnectionSequence(pub ironrdp::connector::ChannelConnectionSequence);

    #[diplomat::opaque]
    pub struct LicenseExchangeSequence(pub ironrdp::connector::LicenseExchangeSequence);
}

#[cfg(test)]
mod tests {
    use ironrdp::pdu::gcc::{ClientClusterData, RedirectionFlags, RedirectionVersion};

    use super::{ClusterRequest, routing_token};

    fn encode(cluster_data: &ClientClusterData) -> Vec<u8> {
        ironrdp::core::encode_vec(cluster_data).expect("cluster data encodes")
    }

    #[test]
    fn no_cluster_request_sends_no_cluster_data() {
        assert!(ClusterRequest::default().cluster_data().is_none());
    }

    #[test]
    fn admin_session_asks_for_session_zero_with_redirection_version_5() {
        let request = ClusterRequest {
            admin_session: true,
            ..ClusterRequest::default()
        };
        let cluster_data = request.cluster_data().expect("an admin session sends cluster data");

        assert_eq!(
            cluster_data,
            ClientClusterData {
                flags: RedirectionFlags::REDIRECTION_SUPPORTED | RedirectionFlags::REDIRECTED_SESSION_FIELD_VALID,
                redirection_version: RedirectionVersion::V5,
                redirected_session_id: 0,
            }
        );
        // Flags: REDIRECTION_SUPPORTED (0x01) | REDIRECTED_SESSIONID_FIELD_VALID (0x02) | version 5 (4 << 2).
        assert_eq!(encode(&cluster_data), [0x13, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn redirected_session_id_takes_precedence_over_the_admin_session() {
        let request = ClusterRequest {
            admin_session: true,
            redirected_session_id: Some(7),
            ..ClusterRequest::default()
        };
        let cluster_data = request.cluster_data().expect("a redirected session sends cluster data");

        assert_eq!(encode(&cluster_data), [0x13, 0, 0, 0, 7, 0, 0, 0]);
    }

    #[test]
    fn redirection_support_alone_names_no_session() {
        let request = ClusterRequest {
            redirection_supported: true,
            ..ClusterRequest::default()
        };
        let cluster_data = request.cluster_data().expect("redirection support sends cluster data");

        assert_eq!(cluster_data.flags, RedirectionFlags::REDIRECTION_SUPPORTED);
        assert_eq!(encode(&cluster_data), [0x11, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn routing_token_drops_one_trailing_crlf() {
        assert_eq!(
            routing_token(b"Cookie: msts=3640655040.15629.0000\r\n").as_deref(),
            Ok("Cookie: msts=3640655040.15629.0000")
        );
        assert_eq!(
            routing_token(b"tsv://MS Terminal Services Plugin.1.Sessions").as_deref(),
            Ok("tsv://MS Terminal Services Plugin.1.Sessions")
        );
    }

    #[test]
    fn routing_token_rejects_what_the_connection_request_cannot_carry() {
        assert!(routing_token(b"").is_err());
        assert!(routing_token(b"\r\n").is_err());
        assert!(routing_token(b"a\r\n\r\n").is_err());
        assert!(routing_token(&[0x01, 0x02]).is_err());
        assert!(routing_token(&[b'a'; 239]).is_err());
        assert!(routing_token(&[b'a'; 238]).is_ok());
    }
}
