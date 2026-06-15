use crate::fix::coinbase::auth::{CoinbaseAuth, CoinbaseAuthError, CoinbaseCredentials};
use crate::types::Side;

use super::{SOH, checksum};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixEncoder {
    begin_string: String,
    sender_comp_id: String,
    target_comp_id: String,
}

impl FixEncoder {
    pub fn new(
        begin_string: impl Into<String>,
        sender_comp_id: impl Into<String>,
        target_comp_id: impl Into<String>,
    ) -> Self {
        Self {
            begin_string: begin_string.into(),
            sender_comp_id: sender_comp_id.into(),
            target_comp_id: target_comp_id.into(),
        }
    }

    pub fn encode_heartbeat(
        &self,
        msg_seq_num: u64,
        sending_time: &str,
        test_req_id: Option<&str>,
    ) -> Vec<u8> {
        let mut body = self.standard_body("0", msg_seq_num, sending_time);
        if let Some(test_req_id) = test_req_id {
            push_field(&mut body, 112, test_req_id);
        }
        self.wrap(body)
    }

    pub fn encode_logon(
        &self,
        msg_seq_num: u64,
        sending_time: &str,
        heart_bt_int_secs: u64,
    ) -> Vec<u8> {
        let mut body = self.standard_body("A", msg_seq_num, sending_time);
        push_field(&mut body, 98, "0");
        push_field(&mut body, 108, &heart_bt_int_secs.to_string());
        self.wrap(body)
    }

    pub fn encode_coinbase_logon(
        &self,
        msg_seq_num: u64,
        sending_time: &str,
        heart_bt_int_secs: u64,
        credentials: &CoinbaseCredentials,
        cancel_on_disconnect: bool,
    ) -> Result<Vec<u8>, CoinbaseAuthError> {
        let signature = CoinbaseAuth::sign_fix_logon(
            credentials,
            sending_time,
            "A",
            msg_seq_num,
            &self.target_comp_id,
        )?;
        let mut body = self.standard_body("A", msg_seq_num, sending_time);
        push_field(&mut body, 98, "0");
        push_field(&mut body, 108, &heart_bt_int_secs.to_string());
        push_field(&mut body, 553, &credentials.api_key);
        push_field(&mut body, 554, &credentials.passphrase);
        push_field(&mut body, 95, &signature.len().to_string());
        push_field(&mut body, 96, &signature);
        push_field(&mut body, 1137, "9");
        if cancel_on_disconnect {
            push_field(&mut body, 8013, "Y");
        }
        Ok(self.wrap(body))
    }

    pub fn encode_market_data_request(
        &self,
        msg_seq_num: u64,
        sending_time: &str,
        md_req_id: &str,
        symbols: &[&str],
    ) -> Vec<u8> {
        self.encode_market_data_request_with_depth(msg_seq_num, sending_time, md_req_id, 1, symbols)
    }

    pub fn encode_market_data_request_with_depth(
        &self,
        msg_seq_num: u64,
        sending_time: &str,
        md_req_id: &str,
        market_depth: u32,
        symbols: &[&str],
    ) -> Vec<u8> {
        let mut body = self.standard_body("V", msg_seq_num, sending_time);
        push_field(&mut body, 262, md_req_id);
        push_field(&mut body, 263, "1");
        push_field(&mut body, 264, &market_depth.to_string());
        push_field(&mut body, 146, &symbols.len().to_string());
        for symbol in symbols {
            push_field(&mut body, 55, symbol);
        }
        self.wrap(body)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn encode_limit_new_order_single(
        &self,
        msg_seq_num: u64,
        sending_time: &str,
        cl_ord_id: &str,
        symbol: &str,
        side: Side,
        price: &str,
        order_qty: &str,
        post_only: bool,
    ) -> Vec<u8> {
        let mut body = self.standard_body("D", msg_seq_num, sending_time);
        push_field(&mut body, 11, cl_ord_id);
        push_field(&mut body, 55, symbol);
        push_field(&mut body, 54, side_to_fix(side));
        push_field(&mut body, 38, order_qty);
        push_field(&mut body, 40, "2");
        push_field(&mut body, 44, price);
        if post_only {
            push_field(&mut body, 18, "6");
        }
        self.wrap(body)
    }

    pub fn encode_order_cancel_request(
        &self,
        msg_seq_num: u64,
        sending_time: &str,
        cl_ord_id: &str,
        orig_cl_ord_id: &str,
        symbol: &str,
        side: Side,
    ) -> Vec<u8> {
        let mut body = self.standard_body("F", msg_seq_num, sending_time);
        push_field(&mut body, 11, cl_ord_id);
        push_field(&mut body, 41, orig_cl_ord_id);
        push_field(&mut body, 55, symbol);
        push_field(&mut body, 54, side_to_fix(side));
        self.wrap(body)
    }

    fn standard_body(&self, msg_type: &str, msg_seq_num: u64, sending_time: &str) -> Vec<u8> {
        let mut body = Vec::new();
        push_field(&mut body, 35, msg_type);
        push_field(&mut body, 34, &msg_seq_num.to_string());
        push_field(&mut body, 49, &self.sender_comp_id);
        push_field(&mut body, 52, sending_time);
        push_field(&mut body, 56, &self.target_comp_id);
        body
    }

    fn wrap(&self, body: Vec<u8>) -> Vec<u8> {
        let mut message = Vec::new();
        push_field(&mut message, 8, &self.begin_string);
        push_field(&mut message, 9, &body.len().to_string());
        message.extend_from_slice(&body);
        let sum = checksum(&message);
        message.extend_from_slice(format!("10={sum:03}").as_bytes());
        message.push(SOH);
        message
    }
}

fn push_field(message: &mut Vec<u8>, tag: u32, value: &str) {
    message.extend_from_slice(tag.to_string().as_bytes());
    message.push(b'=');
    message.extend_from_slice(value.as_bytes());
    message.push(SOH);
}

fn side_to_fix(side: Side) -> &'static str {
    match side {
        Side::Buy => "1",
        Side::Sell => "2",
    }
}
