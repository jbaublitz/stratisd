// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

use std::convert::TryFrom;

use dbus::Message;
use dbus_tree::{MTSync, MethodInfo, MethodResult};

use crate::{
    dbus_api::{
        types::TData,
        util::{engine_to_dbus_err_tuple, get_next_arg, msg_code_ok, msg_string_ok},
    },
    engine::{CreateAction, DeleteAction, KeyDescription},
};

pub fn bind_keyring(m: &MethodInfo<MTSync<TData>, TData>) -> MethodResult {
    let message: &Message = m.msg;
    let mut iter = message.iter_init();
    let key_desc_str: String = get_next_arg(&mut iter, 0)?;

    let dbus_context = m.tree.get_data();
    let object_path = m.path.get_name();
    let return_message = message.method_return();
    let default_return = false;

    let key_desc = match KeyDescription::try_from(key_desc_str) {
        Ok(kd) => kd,
        Err(e) => {
            let (rc, rs) = engine_to_dbus_err_tuple(&e);
            return Ok(vec![return_message.append3(default_return, rc, rs)]);
        }
    };

    let pool_path = m
        .tree
        .get(object_path)
        .expect("implicit argument must be in tree");
    let pool_uuid = typed_uuid!(
        get_data!(pool_path; default_return; return_message).uuid;
        Pool;
        default_return;
        return_message
    );

    let mut mutex_lock = mutex_lock!(dbus_context.engine);
    let (_, pool) = get_mut_pool!(mutex_lock; pool_uuid; default_return; return_message);

    let msg = match log_action!(pool.bind_keyring(&key_desc)) {
        Ok(CreateAction::Identity) => return_message.append3(false, msg_code_ok(), msg_string_ok()),
        Ok(CreateAction::Created(_)) => {
            return_message.append3(true, msg_code_ok(), msg_string_ok())
        }
        Err(e) => {
            let (rc, rs) = engine_to_dbus_err_tuple(&e);
            return_message.append3(default_return, rc, rs)
        }
    };
    Ok(vec![msg])
}

pub fn unbind_keyring(m: &MethodInfo<MTSync<TData>, TData>) -> MethodResult {
    let message: &Message = m.msg;

    let dbus_context = m.tree.get_data();
    let object_path = m.path.get_name();
    let return_message = message.method_return();
    let default_return = false;

    let pool_path = m
        .tree
        .get(object_path)
        .expect("implicit argument must be in tree");
    let pool_uuid = typed_uuid!(
        get_data!(pool_path; default_return; return_message).uuid;
        Pool;
        default_return;
        return_message
    );

    let mut mutex_lock = mutex_lock!(dbus_context.engine);
    let (_, pool) = get_mut_pool!(mutex_lock; pool_uuid; default_return; return_message);

    let msg = match log_action!(pool.unbind_keyring()) {
        Ok(DeleteAction::Identity) => return_message.append3(false, msg_code_ok(), msg_string_ok()),
        Ok(DeleteAction::Deleted(_)) => {
            return_message.append3(true, msg_code_ok(), msg_string_ok())
        }
        Err(e) => {
            let (rc, rs) = engine_to_dbus_err_tuple(&e);
            return_message.append3(default_return, rc, rs)
        }
    };
    Ok(vec![msg])
}
